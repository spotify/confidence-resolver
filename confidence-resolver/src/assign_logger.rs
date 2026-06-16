use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use crate::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use crate::FlagToApply;
use prost::{length_delimiter_len, Message};

mod pb {
    pub use crate::proto::confidence::flags::resolver::v1::events::{
        flag_assigned::{
            self, applied_flag::Assignment, AppliedFlag, AssignmentInfo, DefaultAssignment,
        },
        ClientInfo,
    };
    pub use crate::proto::confidence::flags::resolver::v1::{events::FlagAssigned, ResolveReason};
    pub use flag_assigned::default_assignment::DefaultAssignmentReason;
}

#[derive(Debug, Default)]
struct State {
    pending: VecDeque<(pb::FlagAssigned, usize)>,
    pending_bytes: usize,
}

#[derive(Debug, Default)]
pub struct AssignLogger {
    assigned: crossbeam_queue::SegQueue<pb::FlagAssigned>,
    state: Mutex<State>,
}

impl AssignLogger {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn log_assigns(
        &self,
        resolve_id: &str,
        assigned_flags: &[FlagToApply],
        client: &crate::Client,
        sdk: &Option<crate::flags_resolver::Sdk>,
    ) {
        self.assigned
            .push(build_flag_assigned(resolve_id, assigned_flags, client, sdk));
    }

    pub fn checkpoint(&self) -> WriteFlagLogsRequest {
        let mut req = WriteFlagLogsRequest::default();
        self.checkpoint_fill(&mut req);
        req
    }
    pub fn checkpoint_fill(&self, req: &mut WriteFlagLogsRequest) -> usize {
        self.checkpoint_fill_with_limit(req, usize::MAX, false)
    }

    pub fn checkpoint_with_limit(
        &self,
        limit_bytes: usize,
        require_full: bool,
    ) -> WriteFlagLogsRequest {
        let mut req = WriteFlagLogsRequest::default();
        self.checkpoint_fill_with_limit(&mut req, limit_bytes, require_full);
        req
    }
    pub fn checkpoint_fill_with_limit(
        &self,
        req: &mut WriteFlagLogsRequest,
        limit_bytes: usize,
        require_full: bool,
    ) -> usize {
        let mut state = match self.state.lock() {
            Ok(g) => g,
            // lock errors if another holder panics, still we acquire the lock
            Err(err) => err.into_inner(),
        };
        let start = req.encoded_len();
        let limit_bytes = limit_bytes.saturating_sub(start);
        // Dedup applied flags by (flag, targeting_key, assignment_id) within this flush.
        // Cross-flush dedup could be achieved by moving this set into State and
        // clearing it when state.pending is fully drained after Phase 2.
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        while state.pending_bytes < limit_bytes {
            if let Some(mut assigned) = self.assigned.pop() {
                assigned.flags.retain(|f| {
                    seen.insert((
                        f.flag.clone(),
                        f.targeting_key.clone(),
                        f.assignment_id.clone(),
                    ))
                });
                if assigned.flags.is_empty() {
                    continue;
                }
                let len = AssignLogger::encoded_len(&assigned);
                state.pending.push_back((assigned, len));
                state.pending_bytes = state.pending_bytes.saturating_add(len);
            } else {
                break;
            }
        }
        let mut written: usize = 0;
        if state.pending_bytes >= limit_bytes || !require_full {
            while let Some((_, len)) = state.pending.front() {
                // special case for first event being larger than limit_bytes
                if written.saturating_add(*len) <= limit_bytes || written == 0 && start == 0 {
                    written = written.saturating_add(*len);
                    let assigned = unsafe { state.pending.pop_front().unwrap_unchecked().0 };
                    req.flag_assigned.push(assigned);
                } else {
                    break;
                }
            }
            state.pending_bytes = state.pending_bytes.saturating_sub(written);
        }
        written
    }

    fn encoded_len(assigned: &pb::FlagAssigned) -> usize {
        let len = assigned.encoded_len();
        // the extra one is for the proto type and field id
        len.saturating_add(length_delimiter_len(len))
            .saturating_add(1)
    }
}

pub fn build_flag_assigned(
    resolve_id: &str,
    assigned_flags: &[FlagToApply],
    client: &crate::Client,
    sdk: &Option<crate::flags_resolver::Sdk>,
) -> pb::FlagAssigned {
    let client_info = Some(pb::ClientInfo {
        client: client.client_name.to_string(),
        client_credential: client.client_credential_name.to_string(),
        sdk: sdk.clone(),
    });
    let flags = assigned_flags
        .iter()
        .map(
            |FlagToApply {
                 assigned_flag: f,
                 skew_adjusted_applied_time,
             }| {
                let assignment = if !f.variant.is_empty() {
                    let assignment_info = pb::AssignmentInfo {
                        segment: f.segment.clone(),
                        variant: f.variant.clone(),
                    };
                    Some(pb::Assignment::AssignmentInfo(assignment_info))
                } else {
                    let default_reason: pb::DefaultAssignmentReason =
                        match pb::ResolveReason::try_from(f.reason) {
                            Ok(pb::ResolveReason::NoSegmentMatch) => {
                                pb::DefaultAssignmentReason::NoSegmentMatch
                            }
                            Ok(pb::ResolveReason::NoTreatmentMatch) => {
                                pb::DefaultAssignmentReason::NoTreatmentMatch
                            }
                            Ok(pb::ResolveReason::FlagArchived) => {
                                pb::DefaultAssignmentReason::FlagArchived
                            }
                            _ => pb::DefaultAssignmentReason::Unspecified,
                        };
                    Some(pb::Assignment::DefaultAssignment(pb::DefaultAssignment {
                        reason: default_reason.into(),
                    }))
                };
                pb::AppliedFlag {
                    flag: f.flag.clone(),
                    targeting_key: f.targeting_key.clone(),
                    targeting_key_selector: f.targeting_key_selector.clone(),
                    assignment_id: f.assignment_id.clone(),
                    rule: f.rule.clone(),
                    fallthrough_assignments: f.fallthrough_assignments.clone(),
                    apply_time: Some(skew_adjusted_applied_time.clone()),
                    assignment,
                }
            },
        )
        .collect();

    pb::FlagAssigned {
        resolve_id: resolve_id.to_string(),
        client_info,
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event() -> pb::FlagAssigned {
        pb::FlagAssigned {
            resolve_id: "rid".to_string(),
            client_info: None,
            flags: Vec::new(),
        }
    }

    #[test]
    fn event_size_is_correctly_calculated() {
        let ev = make_event();
        let ev_size = AssignLogger::encoded_len(&ev);
        let req = WriteFlagLogsRequest {
            flag_assigned: vec![ev.clone(), ev],
            ..Default::default()
        };
        assert_eq!(2 * ev_size, req.encoded_len())
    }

    fn make_unique_event(n: usize) -> pb::FlagAssigned {
        make_event_with_flags(
            &format!("rid-{n}"),
            &[(&format!("flags/f-{n}"), "user", "a")],
        )
    }

    #[test]
    fn can_allow_less() {
        let logger = AssignLogger::new();
        logger.assigned.push(make_unique_event(0));

        let r = logger.checkpoint_with_limit(10_000, false);
        assert_eq!(r.flag_assigned.len(), 1);
    }

    #[test]
    fn flushes_until_reaching_target() {
        let ev_size = AssignLogger::encoded_len(&make_unique_event(0));

        let logger = AssignLogger::new();
        logger.assigned.push(make_unique_event(1));
        logger.assigned.push(make_unique_event(2));
        logger.assigned.push(make_unique_event(3));
        let r = logger.checkpoint_with_limit(3 * ev_size - 1, true);
        assert_eq!(r.flag_assigned.len(), 2);
    }

    #[test]
    fn first_event_exceeding_target_is_sent_alone() {
        let logger = AssignLogger::new();
        logger.assigned.push(make_unique_event(0));
        logger.assigned.push(make_unique_event(1));

        let r = logger.checkpoint_with_limit(1, true);
        assert_eq!(r.flag_assigned.len(), 1);
    }

    #[test]
    fn returns_none_when_under_target_and_not_allowed() {
        let logger = AssignLogger::new();
        // no events queued, target positive, allow_less = false
        let r = logger.checkpoint_with_limit(10_000, true);
        assert!(r.flag_assigned.is_empty());
    }

    fn make_event_with_flags(
        resolve_id: &str,
        flags: &[(&str, &str, &str)],
    ) -> pb::FlagAssigned {
        pb::FlagAssigned {
            resolve_id: resolve_id.to_string(),
            client_info: None,
            flags: flags
                .iter()
                .map(|(flag, tk, aid)| pb::AppliedFlag {
                    flag: flag.to_string(),
                    targeting_key: tk.to_string(),
                    assignment_id: aid.to_string(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn dedup_same_flag_targeting_key_assignment() {
        let logger = AssignLogger::new();
        logger
            .assigned
            .push(make_event_with_flags("r1", &[("flags/a", "user1", "ctrl")]));
        logger
            .assigned
            .push(make_event_with_flags("r2", &[("flags/a", "user1", "ctrl")]));
        logger
            .assigned
            .push(make_event_with_flags("r3", &[("flags/a", "user1", "ctrl")]));

        let r = logger.checkpoint();
        assert_eq!(r.flag_assigned.len(), 1);
        assert_eq!(r.flag_assigned[0].flags.len(), 1);
    }

    #[test]
    fn different_targeting_keys_not_deduped() {
        let logger = AssignLogger::new();
        logger
            .assigned
            .push(make_event_with_flags("r1", &[("flags/a", "user1", "ctrl")]));
        logger
            .assigned
            .push(make_event_with_flags("r2", &[("flags/a", "user2", "ctrl")]));

        let r = logger.checkpoint();
        assert_eq!(r.flag_assigned.len(), 2);
    }

    #[test]
    fn different_assignment_ids_not_deduped() {
        let logger = AssignLogger::new();
        logger
            .assigned
            .push(make_event_with_flags("r1", &[("flags/a", "user1", "ctrl")]));
        logger.assigned.push(make_event_with_flags(
            "r2",
            &[("flags/a", "user1", "treatment")],
        ));

        let r = logger.checkpoint();
        assert_eq!(r.flag_assigned.len(), 2);
    }

    #[test]
    fn partial_dedup_within_single_event() {
        let logger = AssignLogger::new();
        logger
            .assigned
            .push(make_event_with_flags("r1", &[("flags/a", "user1", "ctrl")]));
        logger.assigned.push(make_event_with_flags(
            "r2",
            &[("flags/a", "user1", "ctrl"), ("flags/b", "user1", "ctrl")],
        ));

        let r = logger.checkpoint();
        assert_eq!(r.flag_assigned.len(), 2);
        assert_eq!(r.flag_assigned[0].flags.len(), 1);
        assert_eq!(r.flag_assigned[0].flags[0].flag, "flags/a");
        assert_eq!(r.flag_assigned[1].flags.len(), 1);
        assert_eq!(r.flag_assigned[1].flags[0].flag, "flags/b");
    }

    #[test]
    fn separate_flush_calls_both_log() {
        let logger = AssignLogger::new();
        logger
            .assigned
            .push(make_event_with_flags("r1", &[("flags/a", "user1", "ctrl")]));
        let r1 = logger.checkpoint();
        assert_eq!(r1.flag_assigned.len(), 1);

        logger
            .assigned
            .push(make_event_with_flags("r2", &[("flags/a", "user1", "ctrl")]));
        let r2 = logger.checkpoint();
        assert_eq!(r2.flag_assigned.len(), 1);
    }

    #[test]
    #[ignore]
    fn flush_throughput_bench() {
        use std::time::Instant;

        struct Scenario {
            name: &'static str,
            unique_users: usize,
            total_events: usize,
        }

        let scenarios = [
            Scenario {
                name: "0% dup (10k unique)",
                unique_users: 10_000,
                total_events: 10_000,
            },
            Scenario {
                name: "90% dup (1k unique, 10k total)",
                unique_users: 1_000,
                total_events: 10_000,
            },
            Scenario {
                name: "99% dup (100 unique, 10k total)",
                unique_users: 100,
                total_events: 10_000,
            },
            Scenario {
                name: "100% dup (1 unique, 10k total)",
                unique_users: 1,
                total_events: 10_000,
            },
        ];

        for scenario in &scenarios {
            let logger = AssignLogger::new();
            for i in 0..scenario.total_events {
                let user_idx = i % scenario.unique_users;
                logger.assigned.push(make_event_with_flags(
                    &format!("resolve-{i}"),
                    &[("flags/banner", &format!("user-{user_idx}"), "ctrl-assignment")],
                ));
            }

            let start = Instant::now();
            let r = logger.checkpoint();
            let elapsed = start.elapsed();

            eprintln!(
                "[{}] input={}, output_events={}, output_flags={}, elapsed={:?}",
                scenario.name,
                scenario.total_events,
                r.flag_assigned.len(),
                r.flag_assigned.iter().map(|e| e.flags.len()).sum::<usize>(),
                elapsed,
            );
        }
    }
}
