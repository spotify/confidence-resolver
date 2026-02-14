use crate::proto::confidence::flags::admin::v1::client_resolve_info::EvaluationContextSchemaInstance;
use crate::proto::confidence::flags::admin::v1::flag_resolve_info::{
    AssignmentResolveInfo, RuleResolveInfo, VariantResolveInfo,
};
use crate::proto::confidence::flags::admin::v1::{ClientResolveInfo, FlagResolveInfo};
use crate::proto::confidence::flags::resolver::v1::events::FlagAssigned;
use crate::proto::confidence::flags::resolver::v1::{
    ResolveErrorCount, TelemetryData, WriteFlagLogsRequest,
};
use std::collections::{HashMap, HashSet};

pub fn aggregate_batch(message_batch: Vec<WriteFlagLogsRequest>) -> WriteFlagLogsRequest {
    // map of client credential to derived schema
    let mut schema_map: HashMap<String, SchemaItem> = HashMap::new();
    // map of flag to flag resolve info
    let mut flag_resolve_map: HashMap<String, VariantRuleResolveInfo> = HashMap::new();
    let mut flag_assigned: Vec<FlagAssigned> = vec![];
    let mut first_sdk: Option<crate::proto::confidence::flags::resolver::v1::Sdk> = None;
    // Aggregate error counts by error code
    let mut error_counts: HashMap<i32, i64> = HashMap::new();

    for flag_logs_message in message_batch {
        if let Some(td) = &flag_logs_message.telemetry_data {
            if first_sdk.is_none() && td.sdk.is_some() {
                first_sdk = td.sdk.clone();
            }
            // Aggregate error counts
            for error in &td.resolve_errors {
                *error_counts.entry(error.error_code).or_insert(0) += error.count;
            }
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

    // Build resolve_errors from aggregated counts
    let resolve_errors: Vec<ResolveErrorCount> = error_counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(error_code, count)| ResolveErrorCount { error_code, count })
        .collect();

    // Only include TelemetryData if we have SDK info or errors to report
    let telemetry_data = if first_sdk.is_some() || !resolve_errors.is_empty() {
        Some(TelemetryData {
            sdk: first_sdk,
            resolve_errors,
        })
    } else {
        None
    };

    WriteFlagLogsRequest {
        telemetry_data,
        flag_assigned,
        flag_resolve_info,
        client_resolve_info,
    }
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
