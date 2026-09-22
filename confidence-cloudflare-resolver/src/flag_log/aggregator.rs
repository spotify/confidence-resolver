//! The Logpush sink's consumer half: a cron-triggered pass over the R2
//! bucket Logpush writes into.
//!
//! Each object is newline-delimited JSON, one Workers trace event per line,
//! written gzipped. Only console lines carrying the flag-log prefix are
//! parsed — see [`super::logpush::extract`] — so diagnostics, panics and
//! request metadata sharing a trace event are ignored rather than fed to the
//! aggregate.
//!
//! An object is deleted only once its contents have been delivered, so a
//! failed run leaves the work for the next tick. That makes the pass
//! at-least-once with R2 as the durable hand-off and this module owning the
//! retry — the property that the Logpush hop itself does not give you, since
//! it drops a batch permanently after roughly five minutes of failures.
//!
//! The pass is bounded by [`MAX_OBJECTS_PER_RUN`] and [`MAX_OBJECT_BYTES`]
//! rather than draining the bucket, because a Worker has 128 MB of memory and
//! a bounded CPU budget per invocation. Whatever is left over is picked up on the next tick.
use super::{logpush, Sink};
use confidence_resolver::{
    apply_dedup::ApplyDedup, flag_logger,
    proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use flate2::read::MultiGzDecoder;
use std::{cell::RefCell, io::Read};
use worker::{console_log, Bucket, Env};

thread_local! {
    /// Apply-dedup window for the aggregation pass, kept alive between cron
    /// invocations so a duplicate is caught even when its two copies are
    /// drained by different passes.
    ///
    /// Per-isolate, not global: Cloudflare does not pin cron invocations to a
    /// single isolate, so this widens the window on a best-effort basis
    /// rather than guaranteeing it. The resolver's own dedup map has the same
    /// property.
    static DEDUP: RefCell<ApplyDedup> = RefCell::new(super::new_dedup());
}

/// Applies the shared window to one object's logs.
///
/// Borrowed and released inside this call so the `RefCell` is never held
/// across an await in [`collect`].
fn dedup_logs(logs: &mut [WriteFlagLogsRequest], now_seconds: i64) {
    DEDUP.with(|dedup| super::dedup_flag_applies(&mut dedup.borrow_mut(), logs, now_seconds));
}

/// Objects to claim in one pass.
///
/// Bounds CPU, not memory: objects are folded in one at a time, so this is
/// how many decompress-and-parse cycles fit in one invocation. `limits.cpu_ms`
/// is raised by the deployer to match. Whatever is left over is picked up on
/// the next tick, so this throttles a backlog rather than losing it.
const MAX_OBJECTS_PER_RUN: u32 = 200;

/// Reject a single object larger than this once decompressed.
///
/// This is the memory bound, and it is per object because the loop folds each
/// one into the accumulator and drops it — peak usage is one object plus the
/// aggregate, never the sum. The deployer pins the Logpush job's
/// `max_upload_bytes` below this, so a legitimate object always fits and
/// anything over it is a sign something else wrote to the bucket.
const MAX_OBJECT_BYTES: usize = 48 * 1024 * 1024;

/// Runs one aggregation pass. Safe to call under either sink; it returns
/// immediately unless Logpush is the configured one.
pub(crate) async fn run(env: &Env) {
    if super::sink() != Sink::Logpush {
        return;
    }
    let Some(bucket) = super::bucket() else {
        // init() already reported the missing binding.
        return;
    };

    let dedup = super::dedup_enabled(env);
    if dedup {
        // The only thing that evicts: `check_hash` never scans the map, so
        // without this the window fills to its entry cap and silently stops
        // tracking anything new.
        DEDUP.with(|d| d.borrow_mut().sweep((js_sys::Date::now() / 1000.0) as i64));
    }
    let (aggregate, keys, bytes) = collect(&bucket, dedup).await;
    if keys.is_empty() {
        return;
    }

    // Objects holding no flag logs carry nothing recoverable, so they are
    // cleaned up without a delivery attempt. Leaving them would make every
    // later pass re-read them.
    let Some(request) = aggregate else {
        console_log!(
            "flag log aggregation: {} objects held no flag logs, removing",
            keys.len()
        );
        cleanup(&bucket, &keys).await;
        return;
    };

    let delivered = super::deliver(&request).await;
    console_log!(
        "flag log aggregation: {} objects, {} bytes, delivered={}",
        keys.len(),
        bytes,
        delivered
    );

    if delivered {
        cleanup(&bucket, &keys).await;
    } else {
        // Left in place on purpose: the next tick retries them.
        console_log!(
            "flag log aggregation: delivery failed, {} objects retained for retry",
            keys.len()
        );
    }

    if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
        crate::update_kv_snapshot(
            &kv,
            crate::SnapshotPipeline::FlagLogs,
            crate::request_telemetry_to_accumulate(request.telemetry_data.as_ref(), delivered),
            Some(delivered),
            None,
        )
        .await;
    }
}

/// Reads up to the per-run bounds and folds everything into one request.
///
/// Objects are folded in as they are read rather than collected first, so
/// peak memory is one object plus the accumulator instead of every object at
/// once. With at most [`MAX_OBJECTS_PER_RUN`] merges the repeated
/// `aggregate_batch` costs nothing worth avoiding.
async fn collect(
    bucket: &Bucket,
    dedup: bool,
) -> (Option<WriteFlagLogsRequest>, Vec<String>, usize) {
    let listing = match bucket.list().limit(MAX_OBJECTS_PER_RUN).execute().await {
        Ok(listing) => listing,
        Err(e) => {
            console_log!("flag log aggregation: R2 list failed: {:?}", e);
            return (None, Vec::new(), 0);
        }
    };

    let now_seconds = (js_sys::Date::now() / 1000.0) as i64;
    let mut aggregate: Option<WriteFlagLogsRequest> = None;
    let mut keys = Vec::new();
    let mut bytes = 0usize;

    for object in listing.objects() {
        let key = object.key();
        let Some(body) = read_object(bucket, &key).await else {
            // Unreadable: claim it anyway so one poisoned object cannot stall
            // the bucket forever. It contributes nothing to the aggregate.
            keys.push(key);
            continue;
        };
        if body.len() > MAX_OBJECT_BYTES {
            console_log!(
                "flag log aggregation: {} is {} bytes decompressed, over the {} limit; skipping",
                key,
                body.len(),
                MAX_OBJECT_BYTES
            );
            keys.push(key);
            continue;
        }
        bytes = bytes.saturating_add(body.len());

        let (mut logs, skipped) = logpush::extract(&body);
        if skipped > 0 {
            console_log!(
                "flag log aggregation: {} unparseable records skipped in {}",
                skipped,
                key
            );
        }
        keys.push(key);

        if !logs.is_empty() {
            if dedup {
                dedup_logs(&mut logs, now_seconds);
            }
            let merged = flag_logger::aggregate_batch(logs);
            aggregate = Some(match aggregate.take() {
                Some(previous) => flag_logger::aggregate_batch(vec![previous, merged]),
                None => merged,
            });
        }
    }

    (aggregate, keys, bytes)
}

async fn read_object(bucket: &Bucket, key: &str) -> Option<String> {
    let object = match bucket.get(key).execute().await {
        Ok(Some(object)) => object,
        Ok(None) => return None,
        Err(e) => {
            console_log!("flag log aggregation: R2 get {} failed: {:?}", key, e);
            return None;
        }
    };
    let bytes = match object.body() {
        Some(body) => match body.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                console_log!("flag log aggregation: R2 read {} failed: {:?}", key, e);
                return None;
            }
        },
        None => return None,
    };
    match decompress(&bytes) {
        Some(text) => Some(text),
        None => {
            console_log!("flag log aggregation: {} is not readable NDJSON", key);
            None
        }
    }
}

/// Logpush writes R2 objects gzipped, possibly as concatenated members.
/// Plain bytes pass through so an uncompressed object still parses.
fn decompress(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        MultiGzDecoder::new(bytes).read_to_string(&mut text).ok()?;
        return Some(text);
    }
    String::from_utf8(bytes.to_vec()).ok()
}

async fn cleanup(bucket: &Bucket, keys: &[String]) {
    if let Err(e) = bucket.delete_multiple(keys.to_vec()).await {
        // Not fatal: the objects are re-read next tick. Duplicate statistics
        // are the cost of at-least-once, and applies are deduplicated.
        console_log!("flag log aggregation: R2 delete failed: {:?}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn decompresses_gzipped_ndjson() {
        let text = "{\"Logs\":[]}\n{\"Logs\":[]}\n";
        assert_eq!(decompress(&gzip(text)).as_deref(), Some(text));
    }

    #[test]
    fn passes_through_uncompressed_bytes() {
        let text = "{\"Logs\":[]}\n";
        assert_eq!(decompress(text.as_bytes()).as_deref(), Some(text));
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
        let truncated = &bytes[..bytes.len() / 2];

        assert_eq!(decompress(truncated), None);
    }

    #[test]
    fn rejects_non_utf8_uncompressed_bytes() {
        assert_eq!(decompress(&[0xff, 0xfe, 0x00]), None);
    }

    #[test]
    fn empty_object_decompresses_to_nothing_extractable() {
        assert_eq!(decompress(&gzip("")).as_deref(), Some(""));
        assert_eq!(logpush::extract(""), (Vec::new(), 0));
    }
}
