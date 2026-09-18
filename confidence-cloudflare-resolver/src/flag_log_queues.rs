/// Discover the legacy binding followed by contiguous numbered bindings.
pub(crate) fn discover<T>(mut lookup: impl FnMut(&str) -> Option<T>) -> Vec<T> {
    let Some(first) = lookup("flag_logs_queue") else {
        return Vec::new();
    };
    let mut queues = vec![first];
    while let Some(queue) = lookup(&format!("flag_logs_queue_{}", queues.len() + 1)) {
        queues.push(queue);
    }
    queues
}

/// Map a uniform sample in [0, 1) to a queue index.
pub(crate) fn select_index(len: usize, sample: f64) -> usize {
    if len == 0 {
        return 0;
    }
    (sample * len as f64) as usize % len
}

/// Try sending to a randomly selected queue first, then fall through the
/// rest. Returns `true` on the first successful send.
pub(crate) async fn send_to_any(queues: &[worker::Queue], json: &str, sample: f64) -> bool {
    if queues.is_empty() {
        return false;
    }
    let total = queues.len();
    let start = select_index(total, sample);
    for offset in 0..total {
        let idx = (start.wrapping_add(offset)) % total;
        if let Some(queue) = queues.get(idx) {
            match queue.send(json).await {
                Ok(_) => return true,
                Err(e) => {
                    let name = if idx == 0 {
                        "flag_logs_queue".to_string()
                    } else {
                        format!("flag_logs_queue_{}", idx + 1)
                    };
                    if total == 1 {
                        worker::console_log!("queue {} send failed: {:?}", name, e);
                    } else {
                        worker::console_log!(
                            "queue {} send failed (attempt {}/{}): {:?}",
                            name,
                            offset.saturating_add(1),
                            total,
                            e
                        );
                    }
                }
            }
        }
    }
    if total > 1 {
        worker::console_log!("all {} queue shards exhausted, flag log dropped", total);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_legacy_single_queue() {
        let queues = discover(|name| (name == "flag_logs_queue").then_some("original"));
        assert_eq!(queues, ["original"]);
    }

    #[test]
    fn discovers_all_numbered_bindings_in_order() {
        let bindings = [
            ("flag_logs_queue", "original"),
            ("flag_logs_queue_2", "second"),
            ("flag_logs_queue_3", "third"),
            ("events_queue", "events"),
        ];
        let queues = discover(|name| {
            bindings
                .iter()
                .find(|(binding, _)| *binding == name)
                .map(|(_, queue)| *queue)
        });
        assert_eq!(queues, ["original", "second", "third"]);
    }

    #[test]
    fn missing_legacy_binding_disables_logging() {
        let mut looked_up = Vec::new();
        let queues = discover(|name| {
            looked_up.push(name.to_owned());
            (name == "flag_logs_queue_2").then_some("second")
        });
        assert!(queues.is_empty());
        assert_eq!(looked_up, ["flag_logs_queue"]);
    }

    #[test]
    fn stops_at_first_missing_numbered_binding() {
        let queues = discover(|name| match name {
            "flag_logs_queue" => Some("original"),
            "flag_logs_queue_3" => Some("third"),
            _ => None,
        });
        assert_eq!(queues, ["original"]);
    }

    #[test]
    fn empty_returns_zero() {
        assert_eq!(select_index(0, 0.0), 0);
        assert_eq!(select_index(0, 0.999999), 0);
    }

    #[test]
    fn single_queue_always_index_zero() {
        for sample in [0.0, 0.25, 0.5, 0.999999] {
            assert_eq!(select_index(1, sample), 0);
        }
    }

    #[test]
    fn selects_both_sides_of_partition_boundary() {
        for (sample, expected) in [
            (0.0, 0),
            (0.499999, 0),
            (0.5, 1),
            (f64::from_bits(1.0_f64.to_bits() - 1), 1),
        ] {
            assert_eq!(select_index(2, sample), expected);
        }
    }

    #[test]
    fn uniform_samples_distribute_evenly() {
        for count in [2, 3, 8] {
            let mut hits = vec![0usize; count];
            for i in 0..(count * 1000) {
                let sample = (i as f64 + 0.5) / (count * 1000) as f64;
                hits[select_index(count, sample)] += 1;
            }
            assert_eq!(hits, vec![1000; count]);
        }
    }
}
