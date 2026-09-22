//! The Logpush sink's consumer half: one R2 object per queue message.
//!
//! Logpush writes gzipped NDJSON objects into R2, R2 emits an `object-create`
//! notification into a queue, and each notification is handled here as a
//! self-contained unit of work: read, extract, aggregate, deliver, delete.
//!
//! # Why one object per message
//!
//! The work unit is fixed size, so **memory per invocation is constant
//! regardless of total traffic**. That is what makes this scale: throughput
//! is `max_concurrency × records-per-object ÷ delivery-latency`, and every
//! term is a dial rather than a ceiling.
//!
//! The cron pass this replaced aggregated many objects per invocation, and
//! every way it failed traced back to unbounded work per invocation.
//! `flag_logger::aggregate_batch` concatenates `flag_assigned` — exposures
//! are the payload, so they never collapse — meaning the accumulator grew
//! with the backlog until the isolate died. That OOM was unrecoverable:
//! delivery never happened, deletion is delivery-gated, and the next pass
//! read the same objects and died identically. Sizing the unit to the
//! *object* rather than to the backlog removes the whole class.
//!
//! Queues also supply what the cron version hand-rolled and got wrong: one
//! consumer per message instead of a best-effort R2 lease, retries with
//! backoff and a dead letter queue instead of an age-out that silently
//! dropped data, and queue depth as real backpressure instead of a flag in a
//! log line.
//!
//! # Sizing
//!
//! The backend accepts a body up to **4 MiB** — measured by bisection, with
//! `413` above it and no amount of retrying helping. The deployer pins the
//! Logpush job's `max_upload_records` so one object's aggregate lands well
//! inside that, and `max_upload_bytes` high enough that the record count is
//! what binds.
//!
//! Delivery latency measured under load is roughly `1,370 ms + 2.3 ms per
//! assignment`, so larger objects amortise the fixed cost better — up to that
//! hard ceiling. `max_upload_records` is where that trade is expressed.
use super::logpush;
use confidence_resolver::{apply_dedup::ApplyDedup, flag_logger};
use flate2::read::MultiGzDecoder;
use serde::Deserialize;
use std::io::Read;
use worker::{console_log, Bucket, Env, MessageBatch, Result};

/// Reject an object whose decompressed text exceeds this.
///
/// Enforced *during* decompression rather than after, so a malformed or
/// hostile object cannot allocate its way through the isolate before the
/// check runs. The deployer pins the Logpush job's `max_upload_bytes` below
/// this, so a legitimate object always fits.
const MAX_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// One R2 `object-create` notification. Only the fields needed to locate the
/// object are modelled; R2 also sends the account, bucket, size and etag.
#[derive(Deserialize)]
struct ObjectNotification {
    #[serde(default)]
    object: NotificationObject,
    #[serde(default)]
    action: String,
}

#[derive(Deserialize, Default)]
struct NotificationObject {
    #[serde(default)]
    key: String,
}

/// Actions that mean an object was written. R2 notifies on deletes too, and
/// this worker deletes every object it drains, so without this filter each
/// drain would enqueue a second message for a key that no longer exists.
fn is_create(action: &str) -> bool {
    matches!(
        action,
        "PutObject" | "CopyObject" | "CompleteMultipartUpload"
    )
}

/// Handles one batch of object notifications.
///
/// Returning `Err` makes the queue redeliver the batch, which *is* the retry
/// mechanism: a transient delivery failure is retried by the platform with
/// backoff, and an object that keeps failing ends up in the dead letter queue
/// rather than being dropped on a timer.
pub(crate) async fn consume_notifications(batch: MessageBatch<String>, env: &Env) -> Result<()> {
    let Some(bucket) = super::bucket() else {
        console_log!(
            "flag log objects: {} binding is missing; cannot drain",
            super::BUCKET_BINDING
        );
        return Ok(());
    };
    let Ok(messages) = batch.messages() else {
        return Ok(());
    };

    // One window for the whole batch rather than one per object.
    //
    // This is where cross-isolate duplicates are caught, and it is the only
    // place they can be: an object carries records from many isolates, and
    // the resolver's own dedup map only ever sees one isolate's traffic. A
    // batch spans several objects, so sharing the window catches duplicates
    // that straddle them too — at the cost of hashes only, since delivery
    // stays per object.
    let mut dedup = super::dedup_enabled(env).then(super::new_dedup);
    let mut records_delivered = 0usize;
    let mut objects_done = 0usize;
    let mut failures = 0usize;

    for message in messages.iter() {
        let key = match classify(message.body()) {
            Work::Drain(key) => key,
            Work::Skip => continue,
            Work::Unparseable => {
                // Acked rather than retried: it will never become parseable,
                // and redelivering forever would block the queue behind it.
                console_log!(
                    "flag log objects: unparseable notification, skipping: {}",
                    &message.body().chars().take(120).collect::<String>()
                );
                continue;
            }
        };

        match process_object(&bucket, &key, dedup.as_mut()).await {
            Some(records) => {
                objects_done = objects_done.saturating_add(1);
                records_delivered = records_delivered.saturating_add(records);
            }
            None => failures = failures.saturating_add(1),
        }
    }

    console_log!(
        "flag log objects: {} messages, {} objects drained, {} records delivered, {} failed",
        messages.len(),
        objects_done,
        records_delivered,
        failures
    );

    if failures > 0 {
        return Err(worker::Error::RustError(format!(
            "{failures} flag log objects failed delivery"
        )));
    }
    Ok(())
}

/// What a queue message asks for.
#[derive(Debug, PartialEq)]
enum Work {
    /// Drain this object.
    Drain(String),
    /// Nothing to do — a delete notification, or a create with no key.
    Skip,
    /// Not a notification this worker understands.
    Unparseable,
}

/// Classifies one message.
///
/// Pure so it can be tested natively: `console_log!` aborts off wasm32, and
/// the classification is the part worth covering.
fn classify(body: &str) -> Work {
    let Ok(notification) = serde_json::from_str::<ObjectNotification>(body) else {
        return Work::Unparseable;
    };
    if !is_create(&notification.action) || notification.object.key.is_empty() {
        return Work::Skip;
    }
    Work::Drain(notification.object.key)
}

/// Reads, aggregates, delivers and deletes one object.
///
/// `Some(records)` on success, `None` when the batch should be retried. An
/// object that can never succeed — unreadable, or carrying no flag logs — is
/// deleted and counted as done, because redelivering it would block the queue
/// behind something that will not change.
async fn process_object(
    bucket: &Bucket,
    key: &str,
    dedup: Option<&mut ApplyDedup>,
) -> Option<usize> {
    let Some(body) = read_object(bucket, key).await else {
        console_log!("flag log objects: dropping unreadable object {}", key);
        delete(bucket, key).await;
        return Some(0);
    };

    let (mut logs, skipped) = logpush::extract(&body);
    if skipped > 0 {
        console_log!(
            "flag log objects: {} unparseable records in {}",
            skipped,
            key
        );
    }
    if logs.is_empty() {
        delete(bucket, key).await;
        return Some(0);
    }

    let records = logs.len();
    if let Some(dedup) = dedup {
        super::dedup_flag_applies(dedup, &mut logs, (js_sys::Date::now() / 1000.0) as i64);
    }

    let request = flag_logger::aggregate_batch(logs);
    if !super::deliver(&request).await {
        // Left in R2 and reported as a failure, so the queue retries with
        // backoff and eventually dead-letters it.
        return None;
    }

    delete(bucket, key).await;
    Some(records)
}

async fn read_object(bucket: &Bucket, key: &str) -> Option<String> {
    let object = match bucket.get(key).execute().await {
        Ok(Some(object)) => object,
        // Already drained, most likely by a redelivery of the same
        // notification. Not an error.
        Ok(None) => return None,
        Err(e) => {
            console_log!("flag log objects: R2 get {} failed: {:?}", key, e);
            return None;
        }
    };
    let bytes = match object.body() {
        Some(body) => match body.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                console_log!("flag log objects: R2 read {} failed: {:?}", key, e);
                return None;
            }
        },
        None => return None,
    };
    decompress(&bytes)
}

/// Logpush writes R2 objects gzipped, possibly as concatenated members.
/// Plain bytes pass through so an uncompressed object still parses.
fn decompress(bytes: &[u8]) -> Option<String> {
    // One byte over the cap is enough to detect the overrun without
    // materialising more than that.
    let limit = MAX_OBJECT_BYTES as u64 + 1;
    let mut text = String::new();
    let read = if bytes.starts_with(&[0x1f, 0x8b]) {
        MultiGzDecoder::new(bytes)
            .take(limit)
            .read_to_string(&mut text)
    } else {
        bytes.take(limit).read_to_string(&mut text)
    };
    read.ok()?;
    if text.len() > MAX_OBJECT_BYTES {
        return None;
    }
    Some(text)
}

async fn delete(bucket: &Bucket, key: &str) {
    if let Err(e) = bucket.delete(key).await {
        // Not fatal: a redelivery re-reads it, and applies are deduplicated.
        console_log!("flag log objects: R2 delete {} failed: {:?}", key, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    /// The consumer is the only place cross-isolate duplicates can be
    /// caught: an object carries records from many isolates, and the
    /// resolver's own window only ever sees one isolate's traffic.
    #[test]
    fn one_window_spans_the_objects_in_a_batch() {
        use confidence_resolver::proto::confidence::flags::resolver::v1::events::{
            flag_assigned::{applied_flag::Assignment, AppliedFlag, AssignmentInfo},
            FlagAssigned,
        };

        let duplicate = || {
            vec![WriteFlagLogsRequest {
                flag_assigned: vec![FlagAssigned {
                    resolve_id: "r".to_string(),
                    flags: vec![AppliedFlag {
                        flag: "flags/a".to_string(),
                        targeting_key: "user-1".to_string(),
                        assignment: Some(Assignment::AssignmentInfo(AssignmentInfo {
                            variant: "on".to_string(),
                            segment: String::new(),
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }]
        };

        let mut window = super::super::new_dedup();
        let (mut first, mut second) = (duplicate(), duplicate());
        super::super::dedup_flag_applies(&mut window, &mut first, 1000);
        super::super::dedup_flag_applies(&mut window, &mut second, 1000);

        assert_eq!(first[0].flag_assigned.len(), 1, "first copy is kept");
        assert!(
            second[0].flag_assigned.is_empty(),
            "a copy in a later object of the same batch must be dropped"
        );
    }

    #[test]
    fn decompresses_gzipped_ndjson() {
        let text = "{\"Logs\":[]}\n{\"Logs\":[]}\n";
        assert_eq!(decompress(&gzip(text)).as_deref(), Some(text));
    }

    #[test]
    fn passes_through_uncompressed_bytes() {
        assert_eq!(
            decompress(b"{\"Logs\":[]}\n").as_deref(),
            Some("{\"Logs\":[]}\n")
        );
    }

    #[test]
    fn decompresses_concatenated_gzip_members() {
        // Logpush may append members rather than rewriting an object, and a
        // single-member decoder would silently return only the first.
        let mut bytes = gzip("{\"Logs\":[]}\n");
        bytes.extend_from_slice(&gzip("{\"Logs\":[1]}\n"));

        assert_eq!(
            decompress(&bytes).as_deref(),
            Some("{\"Logs\":[]}\n{\"Logs\":[1]}\n")
        );
    }

    #[test]
    fn rejects_truncated_gzip() {
        let bytes = gzip("{\"Logs\":[]}\n");
        assert_eq!(decompress(&bytes[..bytes.len() / 2]), None);
    }

    /// A gzip bomb must be refused without first materialising it, which is
    /// why the cap is applied to the reader rather than to the output.
    #[test]
    fn rejects_an_object_that_decompresses_past_the_cap() {
        let bomb = gzip(&"a".repeat(MAX_OBJECT_BYTES + 1024));
        assert!(bomb.len() < 100_000, "bomb should be small compressed");
        assert_eq!(decompress(&bomb), None);
    }

    #[test]
    fn accepts_an_object_exactly_at_the_cap() {
        let text = "a".repeat(MAX_OBJECT_BYTES);
        assert_eq!(decompress(&gzip(&text)).map(|t| t.len()), Some(text.len()));
    }

    #[test]
    fn reads_the_key_from_an_r2_create_notification() {
        let body = serde_json::json!({
            "account": "abc",
            "action": "PutObject",
            "bucket": "flag-logs",
            "object": {"key": "flag-logs/20260922/x.log.gz", "size": 1234, "eTag": "e"},
            "eventTime": "2026-09-22T16:00:00Z"
        })
        .to_string();

        assert_eq!(
            classify(&body),
            Work::Drain("flag-logs/20260922/x.log.gz".to_string())
        );
    }

    /// This worker deletes every object it drains and R2 notifies on delete,
    /// so without filtering, each drain would enqueue a second message for a
    /// key that no longer exists — doubling queue traffic forever.
    #[test]
    fn delete_notifications_are_not_treated_as_work() {
        let body = serde_json::json!({
            "action": "DeleteObject",
            "object": {"key": "flag-logs/20260922/x.log.gz"}
        })
        .to_string();

        assert_eq!(classify(&body), Work::Skip);
    }

    #[test]
    fn multipart_and_copy_creates_are_work() {
        for action in ["PutObject", "CopyObject", "CompleteMultipartUpload"] {
            let body =
                serde_json::json!({"action": action, "object": {"key": "flag-logs/a"}}).to_string();
            assert_eq!(
                classify(&body),
                Work::Drain("flag-logs/a".to_string()),
                "action = {action}"
            );
        }
    }

    #[test]
    fn tolerates_a_notification_missing_the_object_field() {
        assert_eq!(classify("{\"action\":\"PutObject\"}"), Work::Skip);
    }

    #[test]
    fn a_body_that_is_not_json_is_skipped_rather_than_retried() {
        // Retrying it forever would block the queue behind something that
        // will never parse.
        assert_eq!(classify("not json"), Work::Unparseable);
    }
}
