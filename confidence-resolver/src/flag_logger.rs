use crate::proto::confidence::flags::admin::v1::client_resolve_info::EvaluationContextSchemaInstance;
use crate::proto::confidence::flags::admin::v1::flag_resolve_info::{
    AssignmentResolveInfo, RuleResolveInfo, VariantResolveInfo,
};
use crate::proto::confidence::flags::admin::v1::{ClientResolveInfo, FlagResolveInfo};
use crate::proto::confidence::flags::resolver::v1::events::FlagAssigned;
use crate::proto::confidence::flags::resolver::v1::telemetry_data::ResolveRate;
use crate::proto::confidence::flags::resolver::v1::{TelemetryData, WriteFlagLogsRequest};
use std::collections::{HashMap, HashSet};

pub fn aggregate_batch(message_batch: Vec<WriteFlagLogsRequest>) -> WriteFlagLogsRequest {
    // map of client credential to derived schema
    let mut schema_map: HashMap<String, SchemaItem> = HashMap::new();
    // map of flag to flag resolve info
    let mut flag_resolve_map: HashMap<String, VariantRuleResolveInfo> = HashMap::new();
    let mut flag_assigned: Vec<FlagAssigned> = vec![];
    let mut first_sdk: Option<crate::proto::confidence::flags::resolver::v1::Sdk> = None;
    let mut agg_telemetry: Option<TelemetryData> = None;

    for flag_logs_message in message_batch {
        if let Some(td) = &flag_logs_message.telemetry_data {
            if first_sdk.is_none() && td.sdk.is_some() {
                first_sdk = td.sdk.clone();
            }
            agg_telemetry = Some(merge_telemetry(agg_telemetry.take(), td));
        }

        for c in &flag_logs_message.client_resolve_info {
            if let Some(set) = schema_map.get_mut(&c.client_credential) {
                for schema in &c.schema {
                    set.schemas.insert(schema.clone());
                }
            } else {
                let mut set = HashSet::new();
                for schema in &c.schema {
                    set.insert(schema.clone());
                }
                schema_map.insert(
                    c.client_credential.clone(),
                    SchemaItem {
                        client: c.client.clone(),
                        schemas: set.clone(),
                    },
                );
            }
        }

        for f in &flag_logs_message.flag_resolve_info {
            let flag_info = flag_resolve_map
                .entry(f.flag.clone())
                .or_insert_with(VariantRuleResolveInfo::new);
            update_rule_variant_info(flag_info, f);
        }
        for fa in &flag_logs_message.flag_assigned {
            flag_assigned.push(fa.clone());
        }
    }

    let mut client_resolve_info: Vec<ClientResolveInfo> = vec![];
    for (client_credentials, schema_item) in schema_map {
        client_resolve_info.push(ClientResolveInfo {
            client_credential: client_credentials,
            client: schema_item.client,
            schema: schema_item.schemas.into_iter().collect(),
        })
    }

    let mut flag_resolve_info: Vec<FlagResolveInfo> = vec![];

    for (flag, resolve_info) in flag_resolve_map {
        let variant_resolve_info = resolve_info
            .variant_resolve_info
            .iter()
            .map(|r| VariantResolveInfo {
                variant: r.0.clone(),
                count: *r.1,
            })
            .collect();

        let mut rule_resolve_info: Vec<RuleResolveInfo> = vec![];

        for (rule, info) in resolve_info.rule_resolve_info {
            rule_resolve_info.push(RuleResolveInfo {
                rule,
                count: info.count,
                assignment_resolve_info: info
                    .assignment_count
                    .iter()
                    .map(|(assignment_id, count)| AssignmentResolveInfo {
                        count: *count,
                        assignment_id: assignment_id.clone(),
                    })
                    .collect(),
            });
        }

        flag_resolve_info.push(FlagResolveInfo {
            flag,
            variant_resolve_info,
            rule_resolve_info,
        })
    }

    // Attach SDK info to the aggregated telemetry
    let telemetry_data = match (agg_telemetry, first_sdk) {
        (Some(mut td), sdk) => {
            if td.sdk.is_none() {
                td.sdk = sdk;
            }
            Some(td)
        }
        (None, Some(sdk)) => Some(TelemetryData {
            sdk: Some(sdk),
            ..Default::default()
        }),
        (None, None) => None,
    };

    WriteFlagLogsRequest {
        telemetry_data,
        flag_assigned,
        flag_resolve_info,
        client_resolve_info,
    }
}

/// Merge a telemetry delta into an accumulator.
/// Both are deltas, so counters are summed and gauges take the latest non-zero value.
fn merge_telemetry(acc: Option<TelemetryData>, delta: &TelemetryData) -> TelemetryData {
    let mut acc = acc.unwrap_or_default();

    // Merge resolve latency
    match (&mut acc.resolve_latency, &delta.resolve_latency) {
        (Some(a), Some(d)) => {
            a.sum = a.sum.wrapping_add(d.sum);
            a.count = a.count.wrapping_add(d.count);
            a.buckets.extend(d.buckets.iter().cloned());
            if a.ln_ratio == 0.0 {
                a.ln_ratio = d.ln_ratio;
            }
        }
        (None, Some(d)) => {
            acc.resolve_latency = Some(d.clone());
        }
        _ => {}
    }

    // Merge resolve rates by reason
    for dr in &delta.resolve_rate {
        if let Some(ar) = acc.resolve_rate.iter_mut().find(|r| r.reason == dr.reason) {
            ar.count = ar.count.wrapping_add(dr.count);
        } else {
            acc.resolve_rate.push(ResolveRate {
                count: dr.count,
                reason: dr.reason,
            });
        }
    }

    // Merge provider init rates by label set, mirroring how
    // TelemetrySnapshot::accumulate_delta matches on the full labels map. Kept
    // sorted by label set so the aggregated output is deterministic.
    for dp in &delta.provider_init_rate {
        match acc
            .provider_init_rate
            .iter_mut()
            .find(|entry| entry.labels == dp.labels)
        {
            Some(entry) => entry.count = entry.count.wrapping_add(dp.count),
            None => {
                let idx = acc
                    .provider_init_rate
                    .partition_point(|entry| entry.labels < dp.labels);
                acc.provider_init_rate.insert(idx, dp.clone());
            }
        }
    }

    // Merge apply dedup. applies_*/sweeps are deltas since the last flush so
    // they sum; map_size/map_capacity are point-in-time gauges so they take the
    // latest value rather than accumulating.
    match (&mut acc.apply_dedup, &delta.apply_dedup) {
        (Some(a), Some(d)) => {
            a.applies_total = a.applies_total.wrapping_add(d.applies_total);
            a.applies_deduped = a.applies_deduped.wrapping_add(d.applies_deduped);
            a.apply_dedup_overflow = a.apply_dedup_overflow.wrapping_add(d.apply_dedup_overflow);
            a.sweeps = a.sweeps.wrapping_add(d.sweeps);
            a.map_size = d.map_size;
            a.map_capacity = d.map_capacity;
        }
        (None, Some(d)) => {
            acc.apply_dedup = Some(*d);
        }
        _ => {}
    }

    // Merge flush delivery counters
    match (&mut acc.flush, &delta.flush) {
        (Some(a), Some(d)) => {
            a.succeeded = a.succeeded.wrapping_add(d.succeeded);
            a.failed = a.failed.wrapping_add(d.failed);
        }
        (None, Some(d)) => {
            acc.flush = Some(*d);
        }
        _ => {}
    }

    // Merge event delivery counters
    match (&mut acc.events, &delta.events) {
        (Some(a), Some(d)) => {
            a.published = a.published.wrapping_add(d.published);
            a.batches_succeeded = a.batches_succeeded.wrapping_add(d.batches_succeeded);
            a.batches_failed = a.batches_failed.wrapping_add(d.batches_failed);
            a.events_rejected = a.events_rejected.wrapping_add(d.events_rejected);
        }
        (None, Some(d)) => {
            acc.events = Some(*d);
        }
        _ => {}
    }

    // Gauges: take latest non-zero
    if let Some(sa) = &delta.state_age {
        acc.state_age = Some(*sa);
    }
    if delta.memory_bytes > 0 {
        acc.memory_bytes = delta.memory_bytes;
    }
    if !delta.resolver_version.is_empty() {
        acc.resolver_version = delta.resolver_version.clone();
    }

    acc
}

struct SchemaItem {
    pub client: String,
    pub schemas: HashSet<EvaluationContextSchemaInstance>,
}

#[derive(Debug, Clone)]
struct RuleResolveInfoCount {
    pub count: i64,
    // assignment id to count
    pub assignment_count: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
struct VariantRuleResolveInfo {
    // rule to count
    rule_resolve_info: HashMap<String, RuleResolveInfoCount>,
    // variant to count
    variant_resolve_info: HashMap<String, i64>,
}

impl VariantRuleResolveInfo {
    fn new() -> VariantRuleResolveInfo {
        VariantRuleResolveInfo {
            rule_resolve_info: HashMap::new(),
            variant_resolve_info: HashMap::new(),
        }
    }
}

fn update_rule_variant_info(
    flag_info: &mut VariantRuleResolveInfo,
    rule_resolve_info: &FlagResolveInfo,
) {
    for rule_info in &rule_resolve_info.rule_resolve_info {
        let resolve_count = match flag_info.rule_resolve_info.get(&rule_info.rule) {
            Some(i) => i.count,
            None => 0,
        }
        .saturating_add(rule_info.count);

        // assignment id to count
        let current_assignments: &HashMap<String, i64> =
            match flag_info.rule_resolve_info.get(&rule_info.rule) {
                Some(i) => &i.assignment_count,
                None => &HashMap::new(),
            };

        // assignment id to count
        let mut new_assignment_count: HashMap<String, i64> = HashMap::new();
        for aa in &rule_info.assignment_resolve_info {
            let count = match current_assignments.get(&aa.assignment_id) {
                None => 0,
                Some(a) => *a,
            }
            .saturating_add(aa.count);
            new_assignment_count.insert(aa.clone().assignment_id, count);
        }
        flag_info.rule_resolve_info.insert(
            rule_info.rule.clone(),
            RuleResolveInfoCount {
                count: resolve_count,
                assignment_count: new_assignment_count,
            },
        );
    }

    for variant_info in &rule_resolve_info.variant_resolve_info {
        let count = match flag_info.variant_resolve_info.get(&variant_info.variant) {
            None => 0,
            Some(v) => *v,
        }
        .saturating_add(variant_info.count);
        flag_info
            .variant_resolve_info
            .insert(variant_info.variant.clone(), count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::confidence::flags::resolver::v1::telemetry_data::{
        ApplyDedupTelemetry, EventsTelemetry, FlushTelemetry, ProviderInitRate,
    };
    use std::collections::BTreeMap;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn request(td: TelemetryData) -> WriteFlagLogsRequest {
        WriteFlagLogsRequest {
            telemetry_data: Some(td),
            ..Default::default()
        }
    }

    /// The bug reported on PR #575: aggregate_batch folds a batch together via
    /// merge_telemetry, which only handled latency/rates/state_age/memory/version
    /// and silently discarded everything else. The Cloudflare queue consumer calls
    /// aggregate_batch before both backend delivery and the KV snapshot, so any
    /// field dropped here never reaches either.
    #[test]
    fn aggregate_batch_preserves_all_telemetry_fields() {
        let first = request(TelemetryData {
            apply_dedup: Some(ApplyDedupTelemetry {
                applies_total: 10,
                applies_deduped: 4,
                apply_dedup_overflow: 1,
                sweeps: 2,
                map_size: 100,
                map_capacity: 500,
            }),
            flush: Some(FlushTelemetry {
                succeeded: 3,
                failed: 1,
            }),
            events: Some(EventsTelemetry {
                published: 50,
                batches_succeeded: 2,
                batches_failed: 1,
                events_rejected: 5,
            }),
            provider_init_rate: vec![ProviderInitRate {
                count: 1,
                labels: labels(&[("encryption", "true")]),
            }],
            ..Default::default()
        });

        let second = request(TelemetryData {
            apply_dedup: Some(ApplyDedupTelemetry {
                applies_total: 7,
                applies_deduped: 3,
                apply_dedup_overflow: 2,
                sweeps: 1,
                map_size: 130,
                map_capacity: 500,
            }),
            flush: Some(FlushTelemetry {
                succeeded: 4,
                failed: 2,
            }),
            events: Some(EventsTelemetry {
                published: 20,
                batches_succeeded: 1,
                batches_failed: 3,
                events_rejected: 6,
            }),
            provider_init_rate: vec![
                ProviderInitRate {
                    count: 2,
                    labels: labels(&[("encryption", "true")]),
                },
                ProviderInitRate {
                    count: 5,
                    labels: labels(&[("encryption", "false")]),
                },
            ],
            ..Default::default()
        });

        let agg = aggregate_batch(vec![first, second]);
        let td = agg
            .telemetry_data
            .expect("aggregate_batch produced no telemetry_data at all");

        let dedup = td
            .apply_dedup
            .expect("apply_dedup was dropped by aggregate_batch");
        assert_eq!(dedup.applies_total, 17, "applies_total should sum 10+7");
        assert_eq!(dedup.applies_deduped, 7, "applies_deduped should sum 4+3");
        assert_eq!(
            dedup.apply_dedup_overflow, 3,
            "apply_dedup_overflow should sum 1+2"
        );
        assert_eq!(dedup.sweeps, 3, "sweeps should sum 2+1");
        assert_eq!(
            dedup.map_size, 130,
            "map_size is a gauge and must take the latest reading, not sum to 230"
        );
        assert_eq!(
            dedup.map_capacity, 500,
            "map_capacity is a gauge and must take the latest reading, not sum to 1000"
        );

        let flush = td.flush.expect("flush was dropped by aggregate_batch");
        assert_eq!(flush.succeeded, 7, "flush.succeeded should sum 3+4");
        assert_eq!(flush.failed, 3, "flush.failed should sum 1+2");

        let events = td.events.expect("events was dropped by aggregate_batch");
        assert_eq!(events.published, 70, "events.published should sum 50+20");
        assert_eq!(
            events.batches_succeeded, 3,
            "batches_succeeded should sum 2+1"
        );
        assert_eq!(events.batches_failed, 4, "batches_failed should sum 1+3");
        assert_eq!(events.events_rejected, 11, "events_rejected should sum 5+6");

        assert_eq!(
            td.provider_init_rate.len(),
            2,
            "provider_init_rate should hold one entry per unique label set, got {:?}",
            td.provider_init_rate
        );
        let on = td
            .provider_init_rate
            .iter()
            .find(|e| e.labels == labels(&[("encryption", "true")]))
            .expect("provider_init_rate lost the encryption=true label set");
        assert_eq!(
            on.count, 3,
            "counts for a repeated label set should add (1+2)"
        );
        let off = td
            .provider_init_rate
            .iter()
            .find(|e| e.labels == labels(&[("encryption", "false")]))
            .expect("provider_init_rate lost the encryption=false label set");
        assert_eq!(off.count, 5, "distinct label set should keep its own count");
    }

    /// Gauges must not accumulate even across many deltas.
    #[test]
    fn apply_dedup_gauges_take_latest_while_counters_sum() {
        let batch: Vec<WriteFlagLogsRequest> = (1..=3)
            .map(|i| {
                request(TelemetryData {
                    apply_dedup: Some(ApplyDedupTelemetry {
                        applies_total: 1,
                        applies_deduped: 0,
                        apply_dedup_overflow: 0,
                        sweeps: 0,
                        map_size: i * 10,
                        map_capacity: 64,
                    }),
                    ..Default::default()
                })
            })
            .collect();

        let dedup = aggregate_batch(batch)
            .telemetry_data
            .expect("no telemetry_data")
            .apply_dedup
            .expect("apply_dedup was dropped by aggregate_batch");

        assert_eq!(dedup.applies_total, 3, "counter should sum across 3 deltas");
        assert_eq!(
            dedup.map_size, 30,
            "gauge should be the last reading (30), not the sum (60)"
        );
        assert_eq!(dedup.map_capacity, 64, "constant gauge should stay 64");
    }

    /// A None accumulator must adopt the first delta that carries a value.
    #[test]
    fn aggregate_batch_adopts_first_delta_when_accumulator_empty() {
        let empty = request(TelemetryData::default());
        let carrying = request(TelemetryData {
            apply_dedup: Some(ApplyDedupTelemetry {
                applies_total: 9,
                map_size: 3,
                ..Default::default()
            }),
            flush: Some(FlushTelemetry {
                succeeded: 1,
                failed: 0,
            }),
            events: Some(EventsTelemetry {
                published: 2,
                events_rejected: 1,
                ..Default::default()
            }),
            provider_init_rate: vec![ProviderInitRate {
                count: 1,
                labels: labels(&[("k", "v")]),
            }],
            ..Default::default()
        });

        let td = aggregate_batch(vec![empty, carrying])
            .telemetry_data
            .expect("no telemetry_data");

        assert_eq!(
            td.apply_dedup
                .expect("apply_dedup not adopted")
                .applies_total,
            9
        );
        assert_eq!(td.flush.expect("flush not adopted").succeeded, 1);
        assert_eq!(td.events.expect("events not adopted").events_rejected, 1);
        assert_eq!(td.provider_init_rate.len(), 1);
    }
}
