use std::collections::VecDeque;
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
        _evaluation_context: &crate::proto::google::Struct,
        assigned_flags: &[FlagToApply],
        client: &crate::Client,
        sdk: &Option<crate::flags_resolver::Sdk>,
    ) {
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

        self.assigned.push(pb::FlagAssigned {
            resolve_id: resolve_id.to_string(),
            client_info,
            flags,
        });
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
        while state.pending_bytes < limit_bytes {
            if let Some(assigned) = self.assigned.pop() {
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

    #[test]
    fn can_allow_less() {
        let logger = AssignLogger::new();
        // push a small event directly
        logger.assigned.push(make_event());

        let r = logger.checkpoint_with_limit(10_000, false);
        assert_eq!(r.flag_assigned.len(), 1);
    }

    #[test]
    fn flushes_until_reaching_target() {
        let ev_size = AssignLogger::encoded_len(&make_event());

        let logger = AssignLogger::new(); // tiny target forces immediate flush
                                          // two events
        logger.assigned.push(make_event());
        logger.assigned.push(make_event());
        logger.assigned.push(make_event());
        let r = logger.checkpoint_with_limit(3 * ev_size - 1, true);
        // At least one event should be flushed; with target 0, implementation may flush one
        assert_eq!(r.flag_assigned.len(), 2);
    }

    #[test]
    fn first_event_exceeding_target_is_sent_alone() {
        // Target smaller than single event size
        let logger = AssignLogger::new();
        logger.assigned.push(make_event());
        logger.assigned.push(make_event());

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
}
