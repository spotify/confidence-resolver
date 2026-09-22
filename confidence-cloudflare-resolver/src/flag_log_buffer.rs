//! Statistics aggregation for flag logs.
//!
//! Logs without exposures carry only statistics (`flag_resolve_info`,
//! `client_resolve_info`, `telemetry_data`). One queue message each would
//! waste queue writes, so they are merged into a single in-memory
//! accumulator and flushed on one of two triggers:
//!
//! * the accumulator reaches [`DIRECT_DELIVERY_BYTES`] — it is batch-sized,
//!   so the queue would only add a hop and a write, and it goes straight to
//!   the configured destinations
//! * the coalescing interval elapsed while it was still small — it goes to
//!   the queue, where the consumer batches it with up to 99 others
//!
//! Exposures never wait: they publish immediately, absorbing the accumulator
//! so those statistics ride a write that was happening anyway.
//!
//! Only the queue path is size-constrained (Cloudflare caps messages at
//! 128 KB); direct delivery is an HTTP POST. Since anything reaching
//! [`DIRECT_DELIVERY_BYTES`] bypasses the queue, keeping that constant under
//! the cap is what guarantees every queued message is legal.
//!
//! Delivery is best effort. A queue publish that fails is merged back into
//! the accumulator so the next flush carries it; a direct delivery that
//! fails is logged and dropped, since it is already past the threshold and
//! would immediately flush again.
use confidence_resolver::{
    flag_logger, proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use std::{cell::RefCell, future::Future};
use worker::console_log;

thread_local! {
    static BUFFER: RefCell<Buffer> = RefCell::new(Buffer::default());
}

/// How long the accumulator may keep absorbing logs before it is flushed to
/// the queue.
const DRAIN_INTERVAL_MS: f64 = 200.0;

/// Accumulate until the accumulator reaches this size, then deliver it
/// directly. Must stay under Cloudflare's 128 KB queue message limit: a
/// timer flush happens only below this size, so this bound is what keeps
/// queued messages legal.
const DIRECT_DELIVERY_BYTES: usize = 60_000;

/// Sanity bound on the accumulator, so a failing queue plus repeated
/// restores cannot grow it without limit.
const MAX_MESSAGE_BYTES: usize = 120_000;

#[derive(Default)]
struct Buffer {
    /// The single aggregated accumulator. Taking it out is the claim: the
    /// caller owns it, so no other request can publish it concurrently.
    pending: Option<WriteFlagLogsRequest>,
    pending_size: usize,
    /// `None` until the first log is accumulated, so the coalescing window
    /// starts when there is something to coalesce.
    last_drain_ms: Option<f64>,
}

/// Size as `queue.send(json)` puts it on the wire: the JSON document, then
/// wrapped as a JSON string literal. Unserializable logs report `usize::MAX`
/// so they never appear to fit.
fn wire_size(log: &WriteFlagLogsRequest) -> usize {
    serde_json::to_string(log)
        .and_then(|json| serde_json::to_vec(&json))
        .map_or(usize::MAX, |bytes| bytes.len())
}

fn has_flag_assigns(log: &WriteFlagLogsRequest) -> bool {
    log.flag_assigned
        .iter()
        .any(|assignment| !assignment.flags.is_empty())
}

impl Buffer {
    /// Hand the accumulator to the caller along with its wire size.
    fn take(&mut self) -> Option<(WriteFlagLogsRequest, usize)> {
        let size = self.pending_size;
        self.pending_size = 0;
        self.pending.take().map(|log| (log, size))
    }

    /// Absorb a statistics-only log, then decide whether to flush: the
    /// accumulator having reached the direct-delivery size, or the
    /// coalescing window having elapsed.
    fn offer(
        &mut self,
        log: WriteFlagLogsRequest,
        now_ms: f64,
    ) -> Option<(WriteFlagLogsRequest, usize)> {
        let merged = match self.pending.take() {
            Some(pending) => flag_logger::aggregate_batch(vec![pending, log]),
            None => log,
        };
        self.pending_size = wire_size(&merged);
        self.pending = Some(merged);

        if self.pending_size >= DIRECT_DELIVERY_BYTES {
            self.last_drain_ms = Some(now_ms);
            return self.take();
        }

        let Some(last_drain_ms) = self.last_drain_ms else {
            // First accumulation on this isolate: start the window now.
            self.last_drain_ms = Some(now_ms);
            return None;
        };
        if now_ms - last_drain_ms < DRAIN_INTERVAL_MS {
            return None;
        }
        self.last_drain_ms = Some(now_ms);
        self.take()
    }

    /// Merge a message the queue rejected back into the accumulator so the
    /// next flush carries it. Dropped if that would exceed the hard ceiling.
    fn restore(&mut self, log: WriteFlagLogsRequest) {
        let merged = match self.pending.take() {
            Some(pending) => flag_logger::aggregate_batch(vec![log, pending]),
            None => log,
        };
        let size = wire_size(&merged);
        if size > MAX_MESSAGE_BYTES {
            console_log!("flag log dropped: undeliverable and over size limit");
            self.pending = None;
            self.pending_size = 0;
            return;
        }
        self.pending_size = size;
        self.pending = Some(merged);
    }
}

/// Runs in this request's waitUntil. Performs at most one publish.
pub(crate) async fn send(log: WriteFlagLogsRequest, queues: &[worker::Queue]) {
    send_with(
        log,
        |json| async move {
            super::flag_log_queues::send_to_any(queues, &json, js_sys::Math::random()).await
        },
        |req| async move { deliver_direct(&req).await },
    )
    .await;
}

/// The queue consumer's delivery step, run inline. Whatever reaches here has
/// already been aggregated, so only the delivery walk remains.
async fn deliver_direct(req: &WriteFlagLogsRequest) -> bool {
    let Some(client_secret) = super::CONFIDENCE_CLIENT_SECRET.get() else {
        return false;
    };
    let account_id = super::CDN_STATE_REQUEST.account_id.as_str();
    let destinations = &*super::LOG_DESTINATIONS;
    let Some(&primary) = destinations.first() else {
        return false;
    };

    match super::deliver_flag_logs(client_secret, account_id, req, primary).await {
        Ok(()) => return true,
        Err(reason) => console_log!("direct delivery to {:?} failed: {}", primary, reason),
    }

    let Some(&fallback) = destinations.get(1) else {
        return false;
    };
    match super::deliver_flag_logs(client_secret, account_id, req, fallback).await {
        Ok(()) => true,
        Err(reason) => {
            console_log!("direct delivery to {:?} failed: {}", fallback, reason);
            false
        }
    }
}

async fn send_with<Q, B, FQ, FB>(mut log: WriteFlagLogsRequest, to_queue: Q, to_backend: B)
where
    Q: Fn(String) -> FQ,
    FQ: Future<Output = bool>,
    B: Fn(WriteFlagLogsRequest) -> FB,
    FB: Future<Output = bool>,
{
    // Exposures publish immediately, absorbing the accumulator. No size
    // check is needed: if the result is large it goes direct, and if it is
    // small enough for the queue it is under the message limit by
    // definition.
    let ready = if has_flag_assigns(&log) {
        if let Some((pending, _)) = BUFFER.with(|buffer| buffer.borrow_mut().take()) {
            log = flag_logger::aggregate_batch(vec![log, pending]);
        }
        let size = wire_size(&log);
        Some((log, size))
    } else {
        BUFFER.with(|buffer| buffer.borrow_mut().offer(log, js_sys::Date::now()))
    };

    let Some((req, size)) = ready else { return };

    if size >= DIRECT_DELIVERY_BYTES {
        if !to_backend(req).await {
            console_log!("flag log dropped: direct delivery failed");
        }
        return;
    }

    match serde_json::to_string(&req) {
        Ok(json) => {
            if !to_queue(json).await {
                console_log!("queue publish failed; buffered for the next flush");
                BUFFER.with(|buffer| buffer.borrow_mut().restore(req));
            }
        }
        Err(e) => console_log!("flag log serialize failed: {:?}", e),
    }
}
