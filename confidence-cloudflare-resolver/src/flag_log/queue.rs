//! The queue sink: shard bindings, the producer, and the consumer that
//! aggregates a batch before delivery.
//!
//! Shard discovery and failover live in [`super::shards`].
//!
//! Delivery is best effort on the way in and at-least-once on the way out. A
//! publish runs in the request's `wait_until` and is dropped if every shard
//! rejects it — there is no in-isolate retry, because anything held between
//! requests is lost when the isolate is evicted and Cloudflare offers no
//! shutdown hook to flush it. Once a publish succeeds the queue redelivers to
//! the consumer until it acks.
use super::{dedup_batch_flag_applies, dedup_enabled, shards};
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use std::sync::OnceLock;
use worker::{console_log, Env, MessageBatch, Queue, Result};

static QUEUES: OnceLock<Vec<Queue>> = OnceLock::new();

/// Binds every configured shard. Called only when the queue sink is active,
/// so a buffer deployment does not warn about a binding it does not use.
pub(super) fn init(env: &Env) {
    QUEUES.get_or_init(|| {
        let queues = shards::discover(|name| env.queue(name).ok());
        if queues.is_empty() {
            console_log!("flag_logs_queue binding is missing; logging disabled");
        }
        queues
    });
}

/// Publishes to a randomly chosen shard, falling through the rest on failure.
pub(super) async fn send(log: WriteFlagLogsRequest) {
    match serde_json::to_string(&log) {
        Ok(json) => {
            if let Some(queues) = QUEUES.get() {
                shards::send_to_any(queues, &json, js_sys::Math::random()).await;
            }
        }
        Err(e) => console_log!("flag log serialize failed: {:?}", e),
    }
}

/// Aggregates one queue batch and delivers it, splitting to fit the backend.
///
/// A message that fails to parse is skipped instead of panicking the whole
/// batch, since a panic would retry and eventually drop all of it — including
/// the messages that were fine. Returning `Err` on a failed delivery is
/// deliberate: that is what makes the queue redeliver the batch.
pub(super) async fn consume(message_batch: MessageBatch<String>, env: Env) -> Result<()> {
    let Ok(messages) = message_batch.messages() else {
        return Ok(());
    };

    let mut logs: Vec<WriteFlagLogsRequest> = messages
        .iter()
        .map(|message| message.body().clone())
        .filter_map(|body| match serde_json::from_str(body.as_str()) {
            Ok(log) => Some(log),
            Err(e) => {
                console_log!("flag log message parse failed, skipping: {:?}", e);
                None
            }
        })
        .collect();

    if dedup_enabled(&env) {
        dedup_batch_flag_applies(&mut logs, (js_sys::Date::now() / 1000.0) as i64);
    }

    // Telemetry is merged from the records directly rather than by
    // aggregating a clone of the whole batch just to read one field.
    let telemetry = {
        let mut snap = confidence_resolver::telemetry::TelemetrySnapshot::default();
        let mut any = false;
        for log in &logs {
            if let Some(td) = &log.telemetry_data {
                snap.accumulate_delta(td);
                any = true;
            }
        }
        if any {
            Some(snap)
        } else {
            None
        }
    };
    // Same splitter the buffer uses: a batch of 100 heavy resolves can
    // aggregate past the backend's 4 MiB limit, and a 413 is not retryable,
    // so without splitting it bounces until the dead-letter queue.
    let outcome = super::deliver_all_within_limit(logs).await;
    let delivered = outcome.lost == 0;

    if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
        crate::update_kv_snapshot_merged(
            &kv,
            crate::SnapshotPipeline::FlagLogs,
            // Skipped on a full failure: the queue redelivers the batch and
            // the deltas would be counted twice. A partial success is acked
            // below, so its deltas are counted once and kept.
            if outcome.ok > 0 {
                telemetry.as_ref()
            } else {
                None
            },
            Some(delivered),
        )
        .await;
    }

    if outcome.ok > 0 && outcome.lost > 0 {
        // Partial success. Nacking would redeliver the whole batch and
        // re-post the half that already landed, so ack and report the loss
        // instead: a duplicate exposure is worse than a counted drop, and
        // the split halves cannot be nacked independently.
        // Deliberately not "N of M": a flags split reports the same record
        // on both counters, so ok + lost is not a record total.
        console_log!(
            "flag log: DROPPED {} record(s) after a partial split delivery, {} landed; \
             acking to avoid re-posting them",
            outcome.lost,
            outcome.ok
        );
        return Ok(());
    }

    if !delivered {
        // Nothing landed, so redelivering duplicates nothing.
        return Err(worker::Error::RustError(format!(
            "flag log delivery failed for all {} records",
            outcome.lost
        )));
    }
    Ok(())
}
