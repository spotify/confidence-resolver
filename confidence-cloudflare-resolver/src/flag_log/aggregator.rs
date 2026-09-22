//! The Logpush sink's consumer half: a cron-triggered pass over the R2
//! bucket Logpush writes into.
//!
//! Each object is newline-delimited JSON, one Workers trace event per line,
//! written gzipped. Only console lines carrying the flag-log prefix are
//! parsed — see [`super::logpush::extract`] — so diagnostics, panics and
//! request metadata sharing a trace event are ignored rather than fed to the
//! aggregate.
//!
//! # One object per delivery
//!
//! Each object is read, aggregated, delivered and deleted on its own, and
//! nothing is carried between objects except the apply-dedup window. That is
//! a deliberate choice rather than an efficiency compromise:
//!
//! * **Memory is bounded by one object.** `flag_logger::aggregate_batch`
//!   concatenates `flag_assigned` — exposures are the payload, so they cannot
//!   collapse. A cross-object accumulator therefore grows with the *sum* of
//!   every object in the pass, and decoding expands the wire format several
//!   times over, so a single pass over a backlog would exhaust the isolate's
//!   128 MB. An OOM here would be unrecoverable: the isolate is killed with
//!   nothing catchable, so delivery never happens, and because deletion is
//!   delivery-gated the next pass would read the same objects and die the
//!   same way, stalling permanently while the bucket grew.
//! * **Cost is linear.** Folding pairwise with `aggregate_batch(vec![acc,
//!   next])` re-clones the whole accumulator once per object, which is
//!   quadratic. One call per object is linear.
//! * **A failure is contained.** A body too large or otherwise rejected by
//!   the backend affects one object. With a shared batch, a rejected
//!   aggregate would be retried next pass *plus* whatever arrived meanwhile,
//!   growing monotonically and never succeeding again.
//!
//! Within an object, `aggregate_batch` still collapses its records into a
//! single request, so the delivery count is a small fraction of the log
//! count.
//!
//! # Durability
//!
//! An object is deleted only once its contents have been delivered, so a
//! failed run leaves the work for the next tick. That makes the pass
//! at-least-once with R2 as the durable hand-off and this module owning the
//! retry — the property the Logpush hop itself does not give you, since it
//! drops a batch permanently after roughly five minutes of failures.
//!
//! Retrying forever is its own failure mode, so an object that cannot be
//! delivered before [`MAX_OBJECT_AGE_MS`] is dropped with a loud log. That
//! bounds both the bucket and the blast radius of a permanently rejected
//! payload.
use super::logpush;
use confidence_resolver::{
    apply_dedup::ApplyDedup, flag_logger,
    proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use flate2::read::MultiGzDecoder;
use std::{cell::RefCell, io::Read};
use worker::{console_log, Bucket, Env};

/// Prefixes this worker owns. Listings are restricted to these, so an object
/// the pipeline did not write is never read and — far more importantly —
/// never deleted. The bucket may be one the customer already had.
const PREFIXES: [&str; 2] = ["flag-logs/", "overflow/"];

/// Best-effort pass lease, kept outside [`PREFIXES`] so it is never treated
/// as data.
const LEASE_KEY: &str = "aggregator-lease";

/// How long a lease is honoured. Longer than [`MAX_PASS_MS`] plus delivery,
/// short enough that a crashed pass does not park the bucket for long.
const LEASE_TTL_MS: f64 = 90_000.0;

/// Objects to list per prefix per pass.
const MAX_OBJECTS_PER_PREFIX: u32 = 100;

/// Reject an object whose decompressed text exceeds this.
///
/// Enforced *during* decompression rather than after, so a malformed or
/// hostile object cannot allocate its way through the isolate before the
/// check runs. The deployer pins the Logpush job's `max_upload_bytes` well
/// below this, so a legitimate object always fits.
const MAX_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// Wall-clock budget for one pass.
///
/// Sized so a pass plus its deliveries finishes inside one cron tick, which
/// is what keeps two passes from overlapping and double-counting. Whatever is
/// left over is picked up on the next tick.
const MAX_PASS_MS: f64 = 25_000.0;

/// Give up on an object that has resisted delivery for this long.
const MAX_OBJECT_AGE_MS: f64 = 60.0 * 60.0 * 1000.0;

/// A pass plus its deliveries has to finish inside one cron tick, or two
/// passes overlap and double-count. Checked at compile time so neither
/// constant can drift into an overlapping configuration.
const _: () = assert!(MAX_PASS_MS < 60_000.0);
/// A lease has to outlive the pass holding it, or it stops protecting anything.
const _: () = assert!(LEASE_TTL_MS > MAX_PASS_MS);

thread_local! {
    /// Apply-dedup window, kept alive between cron invocations so a duplicate
    /// is caught even when its two copies are drained by different passes.
    ///
    /// Per-isolate, not global: Cloudflare does not pin cron invocations to a
    /// single isolate, so this widens the window on a best-effort basis
    /// rather than guaranteeing it. The resolver's own dedup map has the same
    /// property.
    static DEDUP: RefCell<ApplyDedup> = RefCell::new(super::new_dedup());
}

/// What one pass did. Reported unconditionally, including the idle case, so
/// "healthy but quiet" is distinguishable from "pipeline dead".
#[derive(Default)]
struct Stats {
    listed: usize,
    delivered_objects: usize,
    delivered_records: usize,
    retained: usize,
    dropped_aged_out: usize,
    dropped_unreadable: usize,
    skipped_records: usize,
    bytes: usize,
    truncated_listing: bool,
    /// Telemetry from delivered objects, folded across the pass.
    ///
    /// Carried separately from the payload because `/metrics` needs it and
    /// KV permits only one write per second per key, so a per-object update
    /// is not an option. Telemetry is small and keyed rather than
    /// concatenated, so folding it is cheap even over hundreds of objects.
    telemetry: Option<WriteFlagLogsRequest>,
}

/// Runs one aggregation pass.
///
/// Deliberately not gated on the active sink. A rollback from `logpush` to
/// `queue` must still finish draining the bucket, and Logpush keeps writing
/// into it for a while after the switch; returning early would leave those
/// objects undelivered and the bucket growing. A queue-only deployment has no
/// bucket bound, so this returns immediately.
pub(crate) async fn run(env: &Env) {
    let Some(bucket) = super::bucket() else {
        return;
    };

    let started_ms = js_sys::Date::now();
    if !claim_lease(&bucket, started_ms).await {
        console_log!("flag log aggregation: another pass holds the lease, skipping");
        return;
    }

    let dedup = super::dedup_enabled(env);
    if dedup {
        // The only thing that evicts: `check_hash` never scans the map, so
        // without this the window fills to its entry cap and silently stops
        // tracking anything new.
        DEDUP.with(|d| d.borrow_mut().sweep((started_ms / 1000.0) as i64));
    }

    let mut stats = Stats::default();
    for prefix in PREFIXES {
        drain_prefix(&bucket, prefix, dedup, started_ms, &mut stats).await;
    }

    // Unconditional: an operator has to be able to tell a quiet deployment
    // from a Logpush job that was disabled, filtered wrong, or auto-disabled
    // by Cloudflare after prolonged failure. Silence used to mean both.
    console_log!(
        "flag log aggregation: listed={} delivered_objects={} delivered_records={} \
         retained={} aged_out={} unreadable={} bad_records={} bytes={} more_pending={} \
         elapsed_ms={}",
        stats.listed,
        stats.delivered_objects,
        stats.delivered_records,
        stats.retained,
        stats.dropped_aged_out,
        stats.dropped_unreadable,
        stats.skipped_records,
        stats.bytes,
        stats.truncated_listing,
        (js_sys::Date::now() - started_ms) as u64
    );

    if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
        let all_delivered = stats.retained == 0 && stats.dropped_aged_out == 0;
        crate::update_kv_snapshot(
            &kv,
            crate::SnapshotPipeline::FlagLogs,
            stats
                .telemetry
                .as_ref()
                .and_then(|t| t.telemetry_data.as_ref()),
            Some(all_delivered),
            None,
        )
        .await;
    }
}

/// Reads a lease object and rewrites it if stale.
///
/// Best effort by construction: R2 offers no conditional put in this binding,
/// so two passes that read the lease within the same instant can both proceed.
/// It narrows the overlap window from the whole pass to the read-write gap,
/// which is what makes double-counting rare rather than routine. Per-object
/// delivery bounds the damage when it does happen to a single object.
async fn claim_lease(bucket: &Bucket, now_ms: f64) -> bool {
    if let Ok(Some(object)) = bucket.get(LEASE_KEY).execute().await {
        if let Some(body) = object.body() {
            if let Ok(text) = body.text().await {
                if let Ok(held_ms) = text.trim().parse::<f64>() {
                    if now_ms - held_ms < LEASE_TTL_MS {
                        return false;
                    }
                }
            }
        }
    }
    if let Err(e) = bucket
        .put(LEASE_KEY, format!("{}", now_ms as u64))
        .execute()
        .await
    {
        // Not fatal: losing the lease only costs overlap protection.
        console_log!("flag log aggregation: lease write failed: {:?}", e);
    }
    true
}

async fn drain_prefix(
    bucket: &Bucket,
    prefix: &str,
    dedup: bool,
    started_ms: f64,
    stats: &mut Stats,
) {
    let listing = match bucket
        .list()
        .prefix(prefix)
        .limit(MAX_OBJECTS_PER_PREFIX)
        .execute()
        .await
    {
        Ok(listing) => listing,
        Err(e) => {
            console_log!("flag log aggregation: R2 list {} failed: {:?}", prefix, e);
            return;
        }
    };
    if listing.truncated() {
        stats.truncated_listing = true;
    }

    for object in listing.objects() {
        if js_sys::Date::now() - started_ms >= MAX_PASS_MS {
            stats.truncated_listing = true;
            return;
        }

        let key = object.key();
        let age_ms = started_ms - object.uploaded().as_millis() as f64;
        stats.listed = stats.listed.saturating_add(1);

        let Some(body) = read_object(bucket, &key).await else {
            // Unreadable is usually a truncated write. Claim it so one
            // poisoned object cannot park the prefix forever.
            console_log!("flag log aggregation: dropping unreadable object {}", key);
            stats.dropped_unreadable = stats.dropped_unreadable.saturating_add(1);
            delete(bucket, &key).await;
            continue;
        };
        stats.bytes = stats.bytes.saturating_add(body.len());

        let (mut logs, skipped) = logpush::extract(&body);
        stats.skipped_records = stats.skipped_records.saturating_add(skipped);
        if skipped > 0 {
            console_log!(
                "flag log aggregation: {} unparseable records in {}",
                skipped,
                key
            );
        }

        if logs.is_empty() {
            // Nothing recoverable in it, so there is nothing to deliver and
            // no reason to re-read it next pass.
            delete(bucket, &key).await;
            continue;
        }

        let records = logs.len();
        if dedup {
            DEDUP.with(|d| {
                super::dedup_flag_applies(
                    &mut d.borrow_mut(),
                    &mut logs,
                    (started_ms / 1000.0) as i64,
                )
            });
        }

        let request = flag_logger::aggregate_batch(logs);
        if super::deliver(&request).await {
            stats.delivered_objects = stats.delivered_objects.saturating_add(1);
            stats.delivered_records = stats.delivered_records.saturating_add(records);
            let shell = WriteFlagLogsRequest {
                telemetry_data: request.telemetry_data,
                ..Default::default()
            };
            stats.telemetry = Some(match stats.telemetry.take() {
                Some(previous) => flag_logger::aggregate_batch(vec![previous, shell]),
                None => shell,
            });
            delete(bucket, &key).await;
        } else if age_ms > MAX_OBJECT_AGE_MS {
            // Retrying indefinitely would grow the bucket without bound and
            // never succeed for a payload the backend refuses outright.
            console_log!(
                "flag log aggregation: DROPPING {} after {}s of failed delivery, \
                 {} records lost",
                key,
                (age_ms / 1000.0) as u64,
                records
            );
            stats.dropped_aged_out = stats.dropped_aged_out.saturating_add(1);
            delete(bucket, &key).await;
        } else {
            // Left in place on purpose: the next tick retries it.
            stats.retained = stats.retained.saturating_add(1);
        }
    }
}

/// Reads and decompresses one object, refusing anything over
/// [`MAX_OBJECT_BYTES`].
///
/// The cap is applied to the decompressing reader rather than to the result,
/// so a highly compressible object cannot allocate past the budget before
/// being rejected.
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
        // Not fatal: the object is re-read next pass. Duplicate statistics
        // are the cost of at-least-once, and applies are deduplicated.
        console_log!("flag log aggregation: R2 delete {} failed: {:?}", key, e);
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
        assert_eq!(decompress(&bytes[..bytes.len() / 2]), None);
    }

    #[test]
    fn rejects_non_utf8_uncompressed_bytes() {
        assert_eq!(decompress(&[0xff, 0xfe, 0x00]), None);
    }

    /// A gzip bomb must be refused without first materialising it, which is
    /// why the cap is applied to the reader rather than to the output.
    #[test]
    fn rejects_an_object_that_decompresses_past_the_cap() {
        let bomb = gzip(&"a".repeat(MAX_OBJECT_BYTES + 1024));
        assert!(
            bomb.len() < 100_000,
            "compressed bomb should be small, was {}",
            bomb.len()
        );

        assert_eq!(decompress(&bomb), None);
    }

    #[test]
    fn accepts_an_object_exactly_at_the_cap() {
        let text = "a".repeat(MAX_OBJECT_BYTES);
        assert_eq!(decompress(&gzip(&text)).map(|t| t.len()), Some(text.len()));
    }

    #[test]
    fn empty_object_yields_nothing_extractable() {
        assert_eq!(decompress(&gzip("")).as_deref(), Some(""));
        assert_eq!(logpush::extract(""), (Vec::new(), 0));
    }

    /// The listing is restricted to prefixes this pipeline writes, because
    /// the bucket may be one the customer already had and every listed key is
    /// eligible for deletion.
    #[test]
    fn owned_prefixes_cover_both_writers_and_exclude_the_lease() {
        assert!(PREFIXES.contains(&"flag-logs/"));
        assert!(PREFIXES.contains(&"overflow/"));
        assert!(
            !PREFIXES.iter().any(|p| LEASE_KEY.starts_with(p)),
            "the lease must never be processed as data"
        );
    }
}
