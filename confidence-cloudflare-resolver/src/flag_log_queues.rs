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

/// Select using a uniform sample in [0, 1), as returned by Math.random().
pub(crate) fn select<T>(queues: &[T], sample: f64) -> Option<&T> {
    queues.get((sample * queues.len() as f64) as usize)
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
    fn empty_queue_list_has_no_destination() {
        assert_eq!(select::<u8>(&[], 0.0), None);
        assert_eq!(select::<u8>(&[], 0.999999), None);
    }

    #[test]
    fn single_queue_always_selected() {
        for sample in [0.0, 0.25, 0.5, 0.999999] {
            assert_eq!(select(&["original"], sample), Some(&"original"));
        }
    }

    #[test]
    fn selects_both_sides_of_partition_boundary() {
        let queues = ["first", "second"];
        for (sample, expected) in [
            (0.0, "first"),
            (0.499999, "first"),
            (0.5, "second"),
            (f64::from_bits(1.0_f64.to_bits() - 1), "second"),
        ] {
            assert_eq!(select(&queues, sample), Some(&expected));
        }
    }

    #[test]
    fn uniform_samples_distribute_evenly_across_all_queues() {
        // Deterministic samples test partitioning without a flaky random test.
        for count in [2, 3, 8] {
            let queues: Vec<_> = (0..count).collect();
            let mut hits = vec![0; count];
            for i in 0..(count * 1000) {
                let sample = (i as f64 + 0.5) / (count * 1000) as f64;
                hits[*select(&queues, sample).unwrap()] += 1;
            }
            assert_eq!(hits, vec![1000; count]);
        }
    }
}
