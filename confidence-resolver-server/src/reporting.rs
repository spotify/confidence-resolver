use crate::{api::WriteFlagLogsRequest, backend::Backend};
use prost::Message;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

const MAX_BATCH_BYTES: usize = 3 * 1024 * 1024;
const MAX_PENDING_REQUESTS: usize = 8192;

struct Pending {
    secret: String,
    logs: WriteFlagLogsRequest,
    samples: crate::sampling::Samples,
    size: usize,
}
#[derive(Default)]
struct Queue {
    items: VecDeque<Pending>,
    bytes: usize,
    count: usize,
}

/// Bounds queued serialized bytes and entry count, including batches being sent.
/// Failed batches keep their reservation until a later retry or explicit drop.
pub struct Reporting {
    queue: Mutex<Queue>,
    flush_lock: tokio::sync::Mutex<()>,
    capacity: usize,
    pub dropped: AtomicU64,
    pub failures: AtomicU64,
}

impl Reporting {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            flush_lock: tokio::sync::Mutex::new(()),
            capacity,
            dropped: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }

    pub fn enqueue(
        &self,
        secret: String,
        logs: WriteFlagLogsRequest,
        samples: crate::sampling::Samples,
    ) {
        if logs.flag_assigned.is_empty()
            && logs.client_resolve_info.is_empty()
            && logs.flag_resolve_info.is_empty()
        {
            return;
        }
        let size =
            crate::sampling::WireLogs::new(logs.clone(), &samples).encoded_len() + secret.len();
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if size > MAX_BATCH_BYTES
            || size > self.capacity.saturating_sub(queue.bytes)
            || queue.count >= MAX_PENDING_REQUESTS
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        queue.bytes += size;
        queue.count += 1;
        queue.items.push_back(Pending {
            secret,
            logs,
            samples,
            size,
        });
    }

    pub fn pending_bytes(&self) -> usize {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).bytes
    }

    pub async fn flush(&self, backend: &Backend) {
        let _flush = self.flush_lock.lock().await;
        // Only process the entries present at the start; new traffic cannot extend a flush forever.
        let mut remaining = self
            .queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .items
            .len();
        while remaining > 0 {
            let groups = {
                let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                let mut groups: BTreeMap<String, Vec<Pending>> = BTreeMap::new();
                let mut bytes = 0;
                while remaining > 0 {
                    let Some(next) = queue.items.front() else {
                        break;
                    };
                    if bytes + next.size > MAX_BATCH_BYTES {
                        break;
                    }
                    let item = queue.items.pop_front().expect("front was present");
                    bytes += item.size;
                    remaining -= 1;
                    groups.entry(item.secret.clone()).or_default().push(item);
                }
                groups
            };
            if groups.is_empty() {
                break;
            }
            for (secret, items) in groups {
                let logs = confidence_resolver::flag_logger::aggregate_batch(
                    items.iter().map(|i| i.logs.clone()).collect(),
                );
                let mut samples = crate::sampling::Samples::new();
                for item in &items {
                    for (credential, fields) in &item.samples {
                        samples
                            .entry(credential.clone())
                            .or_default()
                            .extend(fields.clone());
                    }
                }
                let mut wire = crate::sampling::WireLogs::new(logs.clone(), &samples);
                if wire.encoded_len() > MAX_BATCH_BYTES {
                    wire = crate::sampling::WireLogs::new(logs, &crate::sampling::Samples::new());
                }
                let outcome = backend.send_logs(&secret, wire).await;
                let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                if outcome.is_ok() {
                    queue.bytes -= items.iter().map(|i| i.size).sum::<usize>();
                    queue.count -= items.len();
                } else {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    // Retry on the next scheduled flush. Never log a key, URL or response body.
                    tracing::warn!("Flag log delivery failed; retaining batch for retry");
                    queue.items.extend(items);
                }
            }
        }
    }
}
