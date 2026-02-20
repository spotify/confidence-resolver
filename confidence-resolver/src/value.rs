use chrono::DateTime;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::TimeZone;
use chrono::Utc;

use crate::err::ErrorCode;
use crate::err::Fallible;
use crate::err::OrFailExt;
use crate::proto::google::{value::Kind, Timestamp, Value};
use crate::ResolveError;

use crate::proto::confidence::flags::types::v1::targeting;
use crate::proto::confidence::flags::types::v1::targeting::criterion;

static STRING_VALUE_TYPE: targeting::value::Value =
    targeting::value::Value::StringValue(String::new());

pub fn convert_to_targeting_value(
    attribute_value: &Value,
    expected_type: Option<&targeting::value::Value>,
) -> Fallible<targeting::value::Value> {
    Ok(match &attribute_value.kind {
        None => targeting::value::Value::StringValue("null".to_string()),
        Some(Kind::NullValue(_)) => targeting::value::Value::StringValue("null".to_string()),
        Some(Kind::NumberValue(num_value)) => match expected_type {
            Some(targeting::value::Value::NumberValue(_)) => {
                targeting::value::Value::NumberValue(*num_value)
            }
            Some(targeting::value::Value::StringValue(_)) => {
                targeting::value::Value::StringValue(num_value.to_string())
            }
            _ => targeting::value::Value::StringValue("null".to_string()),
        },
        Some(Kind::StringValue(str_value)) => match expected_type {
            Some(targeting::value::Value::BoolValue(_)) => targeting::value::Value::BoolValue(
                str_value.parse().unwrap_or_else(|_| str_value == "TRUE"),
            ),
            Some(targeting::value::Value::NumberValue(_)) => {
                targeting::value::Value::NumberValue(str_value.parse().or_fail()?)
            } // fixme:propagate error
            Some(targeting::value::Value::StringValue(_)) => {
                targeting::value::Value::StringValue(str_value.clone())
            }
            Some(targeting::value::Value::TimestampValue(_)) => {
                targeting::value::Value::TimestampValue(from_str(str_value).or_fail()?)
            } // fixme:propagate error
            Some(targeting::value::Value::VersionValue(_)) => {
                targeting::value::Value::VersionValue(targeting::SemanticVersion {
                    version: str_value.clone(),
                })
            }
            _ => targeting::value::Value::StringValue("null".to_string()),
        },
        Some(Kind::BoolValue(bool_value)) => match expected_type {
            Some(targeting::value::Value::BoolValue(_)) => {
                targeting::value::Value::BoolValue(*bool_value)
            }
            _ => targeting::value::Value::StringValue("null".to_string()),
        },
        Some(Kind::ListValue(list_value)) => {
            let mut converted_values: Vec<targeting::Value> =
                Vec::with_capacity(list_value.values.len());

            for value in &list_value.values {
                converted_values.push(targeting::Value {
                    value: Some(convert_to_targeting_value(value, expected_type)?),
                });
            }
            targeting::value::Value::ListValue(targeting::ListValue {
                values: converted_values,
            })
        }
        Some(Kind::StructValue(_)) => targeting::value::Value::StringValue("null".to_string()), // todo: fail
    })
}

pub fn evaluate_criterion(
    attribute_criterion: &criterion::AttributeCriterion,
    wrapped: &targeting::ListValue,
) -> Result<bool, ResolveError> {
    let Some(rule) = &attribute_criterion.rule else {
        return Err(ResolveError::UnrecognizedRule);
    };
    let context_values = &wrapped.values;
    match rule {
        criterion::attribute_criterion::Rule::EqRule(targeting::EqRule { value: Some(value) }) => {
            Ok(context_values.contains(value))
        }
        criterion::attribute_criterion::Rule::SetRule(targeting::SetRule { values }) => {
            Ok(context_values.iter().any(|v| values.contains(v)))
        }
        criterion::attribute_criterion::Rule::RangeRule(range_rule) => Ok(context_values
            .iter()
            .any(|v| evaluate_range_rule(range_rule, v))),
        criterion::attribute_criterion::Rule::AnyRule(targeting::AnyRule {
            rule: Some(inner_rule),
        }) => {
            for v in context_values {
                if evaluate_inner_rule(inner_rule, v)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        criterion::attribute_criterion::Rule::AllRule(targeting::AllRule {
            rule: Some(inner_rule),
        }) => {
            for v in context_values {
                if !evaluate_inner_rule(inner_rule, v)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        criterion::attribute_criterion::Rule::StartsWithRule(targeting::StartsWithRule {
            value,
        }) => Ok(context_values.iter().any(|v| starts_with(v, value))),
        criterion::attribute_criterion::Rule::EndsWithRule(targeting::EndsWithRule { value }) => {
            Ok(context_values.iter().any(|v| ends_with(v, value)))
        }
        _ => Err(ResolveError::UnrecognizedRule),
    }
}

fn evaluate_inner_rule(
    inner_rule: &targeting::InnerRule,
    context_value: &targeting::Value,
) -> Result<bool, ResolveError> {
    let Some(rule) = &inner_rule.rule else {
        return Err(ResolveError::UnrecognizedRule);
    };
    match rule {
        targeting::inner_rule::Rule::EqRule(targeting::EqRule { value: Some(value) }) => {
            Ok(context_value == value)
        }
        targeting::inner_rule::Rule::SetRule(targeting::SetRule { values }) => {
            Ok(values.contains(context_value))
        }
        targeting::inner_rule::Rule::RangeRule(range_rule) => {
            Ok(evaluate_range_rule(range_rule, context_value))
        }
        targeting::inner_rule::Rule::StartsWithRule(targeting::StartsWithRule { value }) => {
            Ok(starts_with(context_value, value))
        }
        targeting::inner_rule::Rule::EndsWithRule(targeting::EndsWithRule { value }) => {
            Ok(ends_with(context_value, value))
        }
        _ => Err(ResolveError::UnrecognizedRule),
    }
}

fn starts_with(value: &targeting::Value, prefix: &str) -> bool {
    matches!(&value.value, Some(targeting::value::Value::StringValue(s)) if s.starts_with(prefix))
}

fn ends_with(value: &targeting::Value, suffix: &str) -> bool {
    matches!(&value.value, Some(targeting::value::Value::StringValue(s)) if s.ends_with(suffix))
}

fn evaluate_range_rule(
    range_rule: &targeting::RangeRule,
    context_value: &targeting::Value,
) -> bool {
    let after_start = match &range_rule.start {
        Some(targeting::range_rule::Start::StartInclusive(start_inclusive)) => {
            start_inclusive.lte(context_value)
        }
        Some(targeting::range_rule::Start::StartExclusive(start_exclusive)) => {
            start_exclusive.lt(context_value)
        }
        _ => false,
    };

    let before_end = match &range_rule.end {
        Some(targeting::range_rule::End::EndInclusive(end_inclusive)) => {
            context_value.lte(end_inclusive)
        }
        Some(targeting::range_rule::End::EndExclusive(end_exclusive)) => {
            context_value.lt(end_exclusive)
        }
        _ => false,
    };

    (range_rule.start.is_none() || after_start) && (range_rule.end.is_none() || before_end)
}

trait Ord {
    fn lt(&self, other: &Self) -> bool;
    fn lte(&self, other: &Self) -> bool;
}

impl Ord for targeting::Value {
    fn lt(&self, other: &Self) -> bool {
        let Some(a) = &self.value else { return false };
        let Some(b) = &other.value else { return false };
        match (a, b) {
            (targeting::value::Value::NumberValue(a), targeting::value::Value::NumberValue(b)) => {
                a < b
            }
            (targeting::value::Value::StringValue(a), targeting::value::Value::StringValue(b)) => {
                a < b
            }
            (
                targeting::value::Value::TimestampValue(a),
                targeting::value::Value::TimestampValue(b),
            ) => a.lt(b),
            (
                targeting::value::Value::VersionValue(a),
                targeting::value::Value::VersionValue(b),
            ) => a.lt(b),
            _ => false,
        }
    }

    fn lte(&self, other: &Self) -> bool {
        let Some(a) = &self.value else { return false };
        let Some(b) = &other.value else { return false };
        match (a, b) {
            (targeting::value::Value::NumberValue(a), targeting::value::Value::NumberValue(b)) => {
                a <= b
            }
            (targeting::value::Value::StringValue(a), targeting::value::Value::StringValue(b)) => {
                a <= b
            }
            (
                targeting::value::Value::TimestampValue(a),
                targeting::value::Value::TimestampValue(b),
            ) => a.lte(b),
            (
                targeting::value::Value::VersionValue(a),
                targeting::value::Value::VersionValue(b),
            ) => a.lte(b),
            _ => false,
        }
    }
}

impl Ord for Timestamp {
    fn lt(&self, other: &Self) -> bool {
        if self.seconds < other.seconds {
            true
        } else if self.seconds == other.seconds {
            self.nanos < other.nanos
        } else {
            false
        }
    }

    fn lte(&self, other: &Self) -> bool {
        if self.seconds < other.seconds {
            true
        } else if self.seconds == other.seconds {
            self.nanos <= other.nanos
        } else {
            false
        }
    }
}

const ZERO_VERSION: semver::Version = semver::Version::new(0, 0, 0);

impl Ord for targeting::SemanticVersion {
    fn lt(&self, other: &Self) -> bool {
        // this use of ZERO_VERSION is questionable
        let a = semver::Version::parse(&self.version).unwrap_or(ZERO_VERSION);
        let b = semver::Version::parse(&other.version).unwrap_or(ZERO_VERSION);
        a < b
    }

    fn lte(&self, other: &Self) -> bool {
        // this use of ZERO_VERSION is questionable
        let a = semver::Version::parse(&self.version).unwrap_or(ZERO_VERSION);
        let b = semver::Version::parse(&other.version).unwrap_or(ZERO_VERSION);
        a <= b
    }
}

fn from_str(s: &str) -> Fallible<Timestamp> {
    // parse timestamp from s
    if s.contains(['T', ' ']) {
        // split at position of T or space
        let time_part = s.split(['T', ' ']).nth(1).or_fail()?;
        if time_part.contains(['Z', '+', '-']) {
            DateTime::parse_from_rfc3339(s)
                .or_fail()
                .map(|dt| dt.with_timezone(&Utc))
                .map(|dt| Timestamp {
                    seconds: dt.timestamp(),
                    nanos: dt.timestamp_subsec_nanos() as i32,
                })
        } else {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f"))
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
                .or_fail()
                .and_then(|ndt| match Utc.from_local_datetime(&ndt) {
                    LocalResult::Single(dt) => Ok(dt),
                    _ => Err(ErrorCode::from_location()),
                })
                .map(|dt| Timestamp {
                    seconds: dt.timestamp(),
                    nanos: dt.timestamp_subsec_nanos() as i32,
                })
        }
    } else {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .or_fail()
            .map(|nd| unsafe { nd.and_hms_opt(0, 0, 0).unwrap_unchecked() })
            .and_then(|ndt| match Utc.from_local_datetime(&ndt) {
                chrono::LocalResult::Single(dt) => Ok(dt),
                _ => Err(ErrorCode::from_location()),
            })
            .map(|dt| Timestamp {
                seconds: dt.timestamp(),
                nanos: dt.timestamp_subsec_nanos() as i32,
            })
    }
}

pub fn expected_value_type(
    attribute_criterion: &targeting::criterion::AttributeCriterion,
) -> Option<&targeting::value::Value> {
    attribute_criterion.expected_value_type()
}

trait ExpectedValueType {
    fn expected_value_type(&self) -> Option<&targeting::value::Value>;
}

impl ExpectedValueType for targeting::criterion::AttributeCriterion {
    fn expected_value_type(&self) -> Option<&targeting::value::Value> {
        match self.rule.as_ref()? {
            criterion::attribute_criterion::Rule::EqRule(eq_rule) => eq_rule.expected_value_type(),
            criterion::attribute_criterion::Rule::SetRule(set_rule) => {
                set_rule.expected_value_type()
            }
            criterion::attribute_criterion::Rule::RangeRule(range_rule) => {
                range_rule.expected_value_type()
            }
            criterion::attribute_criterion::Rule::AnyRule(any_rule) => {
                any_rule.rule.as_ref()?.expected_value_type()
            }
            criterion::attribute_criterion::Rule::AllRule(all_rule) => {
                all_rule.rule.as_ref()?.expected_value_type()
            }
            criterion::attribute_criterion::Rule::StartsWithRule(_)
            | criterion::attribute_criterion::Rule::EndsWithRule(_) => Some(&STRING_VALUE_TYPE),
        }
    }
}

impl ExpectedValueType for targeting::InnerRule {
    fn expected_value_type(&self) -> Option<&targeting::value::Value> {
        match self.rule.as_ref()? {
            targeting::inner_rule::Rule::EqRule(eq_rule) => eq_rule.expected_value_type(),
            targeting::inner_rule::Rule::SetRule(set_rule) => set_rule.expected_value_type(),
            targeting::inner_rule::Rule::RangeRule(range_rule) => range_rule.expected_value_type(),
            targeting::inner_rule::Rule::StartsWithRule(_)
            | targeting::inner_rule::Rule::EndsWithRule(_) => Some(&STRING_VALUE_TYPE),
        }
    }
}

impl ExpectedValueType for targeting::EqRule {
    fn expected_value_type(&self) -> Option<&targeting::value::Value> {
        self.value.as_ref()?.value.as_ref()
    }
}
impl ExpectedValueType for targeting::SetRule {
    fn expected_value_type(&self) -> Option<&targeting::value::Value> {
        self.values.first().as_ref()?.value.as_ref()
    }
}
impl ExpectedValueType for targeting::RangeRule {
    fn expected_value_type(&self) -> Option<&targeting::value::Value> {
        match (&self.start, &self.end) {
            (Some(targeting::range_rule::Start::StartInclusive(value)), _) => value.value.as_ref(),
            (Some(targeting::range_rule::Start::StartExclusive(value)), _) => value.value.as_ref(),
            (_, Some(targeting::range_rule::End::EndInclusive(value))) => value.value.as_ref(),
            (_, Some(targeting::range_rule::End::EndExclusive(value))) => value.value.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[cfg(test)]
    macro_rules! bool_type {
        () => {
            Some(&targeting::value::Value::BoolValue(false))
        };
    }
    #[cfg(test)]
    macro_rules! number_type {
        () => {
            Some(&targeting::value::Value::NumberValue(0.0))
        };
    }
    #[cfg(test)]
    macro_rules! string_type {
        () => {
            Some(&targeting::value::Value::StringValue("".to_string()))
        };
    }
    #[cfg(test)]
    macro_rules! timestamp_type {
        () => {
            Some(&targeting::value::Value::TimestampValue(
                Timestamp::default(),
            ))
        };
    }
    #[cfg(test)]
    macro_rules! version_type {
        () => {
            Some(&targeting::value::Value::VersionValue(
                targeting::SemanticVersion {
                    version: "".to_string(),
                },
            ))
        };
    }

    #[test]
    fn convert_number_to_number() {
        let number = convert_to_targeting_value(&123.4.into(), number_type!()).unwrap();
        assert_number(&number, 123.4);
    }

    #[test]
    fn convert_number_to_string() {
        let number = convert_to_targeting_value(&123.4.into(), string_type!()).unwrap();
        assert_string(&number, "123.4");
    }

    #[test]
    fn convert_string_to_bool() {
        let bool_tl = convert_to_targeting_value(&"true".into(), bool_type!()).unwrap();
        let bool_tu = convert_to_targeting_value(&"TRUE".into(), bool_type!()).unwrap();
        let bool_fl = convert_to_targeting_value(&"false".into(), bool_type!()).unwrap();
        let bool_fu = convert_to_targeting_value(&"FALSE".into(), bool_type!()).unwrap();
        let bool_rnd = convert_to_targeting_value(&"rnd".into(), bool_type!()).unwrap();

        assert_bool(&bool_tl, true);
        assert_bool(&bool_tu, true);
        assert_bool(&bool_fl, false);
        assert_bool(&bool_fu, false);
        assert_bool(&bool_rnd, false);
    }

    #[test]
    fn convert_string_to_number() {
        let number1 = convert_to_targeting_value(&"123".into(), number_type!()).unwrap();
        let number2 = convert_to_targeting_value(&"123.4".into(), number_type!()).unwrap();

        assert_number(&number1, 123.0);
        assert_number(&number2, 123.4);
    }

    #[test]
    fn convert_string_to_string() {
        let str = convert_to_targeting_value(&"foobar".into(), string_type!()).unwrap();

        assert_string(&str, "foobar");
    }

    #[test]
    fn convert_string_to_timestamp() {
        let time = "2022-11-17T15:16:17.118Z";
        let timestamp = convert_to_targeting_value(&time.into(), timestamp_type!()).unwrap();

        let expected = chrono::DateTime::parse_from_rfc3339(time).unwrap();
        assert_timestamp(&timestamp, &expected);
    }

    #[test]
    fn convert_string_to_timestamp_no_t() {
        let time = "2022-11-17 15:16:17.118Z";
        let timestamp = convert_to_targeting_value(&time.into(), timestamp_type!()).unwrap();

        let expected = chrono::DateTime::parse_from_rfc3339(time).unwrap();
        assert_timestamp(&timestamp, &expected);
    }

    #[test]
    fn convert_string_to_timestamp_no_zone() {
        let timestamp1 =
            convert_to_targeting_value(&"2022-11-17T15:16:17.118".into(), timestamp_type!())
                .unwrap();
        let timestamp2 =
            convert_to_targeting_value(&"2022-11-17 15:16:17.118".into(), timestamp_type!())
                .unwrap();
        let timestamp3 =
            convert_to_targeting_value(&"2022-11-17T15:16:17".into(), timestamp_type!()).unwrap();
        let timestamp4 =
            convert_to_targeting_value(&"2022-11-17 15:16:17".into(), timestamp_type!()).unwrap();

        let expected_with_nanos =
            chrono::DateTime::parse_from_rfc3339("2022-11-17T15:16:17.118Z").unwrap();
        let expected_zero_nanos =
            chrono::DateTime::parse_from_rfc3339("2022-11-17T15:16:17.000Z").unwrap();
        assert_timestamp(&timestamp1, &expected_with_nanos);
        assert_timestamp(&timestamp2, &expected_with_nanos);
        assert_timestamp(&timestamp3, &expected_zero_nanos);
        assert_timestamp(&timestamp4, &expected_zero_nanos);
    }

    #[test]
    fn convert_string_to_timestamp_zoned() {
        let time = "2022-11-17T15:16:17+01:00";
        let timestamp = convert_to_targeting_value(&time.into(), timestamp_type!()).unwrap();

        let expected = chrono::DateTime::parse_from_rfc3339("2022-11-17T14:16:17Z").unwrap();
        assert_timestamp(&timestamp, &expected);
    }

    #[test]
    fn convert_string_to_timestamp_zoned_negative() {
        let time = "2022-11-17T15:16:17-01:00";
        let timestamp = convert_to_targeting_value(&time.into(), timestamp_type!()).unwrap();

        let expected = chrono::DateTime::parse_from_rfc3339("2022-11-17T16:16:17Z").unwrap();
        assert_timestamp(&timestamp, &expected);
    }

    #[test]
    fn convert_string_to_timestamp_date() {
        let time = "2022-11-17";
        let timestamp = convert_to_targeting_value(&time.into(), timestamp_type!()).unwrap();

        let expected = chrono::DateTime::parse_from_rfc3339("2022-11-17T00:00:00Z").unwrap();
        assert_timestamp(&timestamp, &expected);
    }

    #[test]
    fn convert_string_to_version() {
        let version = convert_to_targeting_value(&"4.16.2".into(), version_type!()).unwrap();

        assert_version(&version, "4.16.2");
    }

    #[test]
    fn convert_bool_to_bool() {
        let bool_t = convert_to_targeting_value(&true.into(), bool_type!()).unwrap();
        let bool_f = convert_to_targeting_value(&false.into(), bool_type!()).unwrap();

        assert_bool(&bool_t, true);
        assert_bool(&bool_f, false);
    }

    fn make_string_value(s: &str) -> targeting::Value {
        targeting::Value {
            value: Some(targeting::value::Value::StringValue(s.to_string())),
        }
    }

    fn make_list_value(values: Vec<targeting::Value>) -> targeting::ListValue {
        targeting::ListValue { values }
    }

    fn make_starts_with_criterion(prefix: &str) -> criterion::AttributeCriterion {
        criterion::AttributeCriterion {
            attribute_name: "attr".to_string(),
            rule: Some(criterion::attribute_criterion::Rule::StartsWithRule(
                targeting::StartsWithRule {
                    value: prefix.to_string(),
                },
            )),
        }
    }

    fn make_ends_with_criterion(suffix: &str) -> criterion::AttributeCriterion {
        criterion::AttributeCriterion {
            attribute_name: "attr".to_string(),
            rule: Some(criterion::attribute_criterion::Rule::EndsWithRule(
                targeting::EndsWithRule {
                    value: suffix.to_string(),
                },
            )),
        }
    }

    // --- startsWith tests (mirrors Java EvalUtilTest) ---

    #[test]
    fn starts_with_match() {
        let list = make_list_value(vec![make_string_value("hello-world")]);
        assert!(evaluate_criterion(&make_starts_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn starts_with_no_match() {
        let list = make_list_value(vec![make_string_value("hello-world")]);
        assert!(!evaluate_criterion(&make_starts_with_criterion("world"), &list).unwrap());
    }

    #[test]
    fn starts_with_exact_match() {
        let list = make_list_value(vec![make_string_value("hello")]);
        assert!(evaluate_criterion(&make_starts_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn starts_with_empty_prefix() {
        let list = make_list_value(vec![make_string_value("hello")]);
        assert!(evaluate_criterion(&make_starts_with_criterion(""), &list).unwrap());
    }

    #[test]
    fn starts_with_empty_value_and_empty_prefix() {
        let list = make_list_value(vec![make_string_value("")]);
        assert!(evaluate_criterion(&make_starts_with_criterion(""), &list).unwrap());
    }

    #[test]
    fn starts_with_no_value() {
        let list = make_list_value(vec![targeting::Value { value: None }]);
        assert!(!evaluate_criterion(&make_starts_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn starts_with_empty_list() {
        let list = make_list_value(vec![]);
        assert!(!evaluate_criterion(&make_starts_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn starts_with_non_string_value() {
        let list = make_list_value(vec![targeting::Value {
            value: Some(targeting::value::Value::NumberValue(42.0)),
        }]);
        assert!(!evaluate_criterion(&make_starts_with_criterion("4"), &list).unwrap());
    }

    // --- endsWith tests (mirrors Java EvalUtilTest) ---

    #[test]
    fn ends_with_match() {
        let list = make_list_value(vec![make_string_value("hello-world")]);
        assert!(evaluate_criterion(&make_ends_with_criterion("world"), &list).unwrap());
    }

    #[test]
    fn ends_with_no_match() {
        let list = make_list_value(vec![make_string_value("hello-world")]);
        assert!(!evaluate_criterion(&make_ends_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn ends_with_exact_match() {
        let list = make_list_value(vec![make_string_value("world")]);
        assert!(evaluate_criterion(&make_ends_with_criterion("world"), &list).unwrap());
    }

    #[test]
    fn ends_with_empty_suffix() {
        let list = make_list_value(vec![make_string_value("hello")]);
        assert!(evaluate_criterion(&make_ends_with_criterion(""), &list).unwrap());
    }

    #[test]
    fn ends_with_empty_value_and_empty_suffix() {
        let list = make_list_value(vec![make_string_value("")]);
        assert!(evaluate_criterion(&make_ends_with_criterion(""), &list).unwrap());
    }

    #[test]
    fn ends_with_no_value() {
        let list = make_list_value(vec![targeting::Value { value: None }]);
        assert!(!evaluate_criterion(&make_ends_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn ends_with_empty_list() {
        let list = make_list_value(vec![]);
        assert!(!evaluate_criterion(&make_ends_with_criterion("hello"), &list).unwrap());
    }

    #[test]
    fn ends_with_non_string_value() {
        let list = make_list_value(vec![targeting::Value {
            value: Some(targeting::value::Value::NumberValue(42.0)),
        }]);
        assert!(!evaluate_criterion(&make_ends_with_criterion("2"), &list).unwrap());
    }

    // --- list-level tests (Rust-specific, evaluate_criterion works on lists) ---

    #[test]
    fn starts_with_any_value_in_list_matches() {
        let list = make_list_value(vec![
            make_string_value("user@gmail.com"),
            make_string_value("admin@example.com"),
        ]);
        assert!(evaluate_criterion(&make_starts_with_criterion("admin"), &list).unwrap());
    }

    #[test]
    fn starts_with_no_value_in_list_matches() {
        let list = make_list_value(vec![
            make_string_value("user@gmail.com"),
            make_string_value("user@example.com"),
        ]);
        assert!(!evaluate_criterion(&make_starts_with_criterion("admin"), &list).unwrap());
    }

    #[test]
    fn ends_with_any_value_in_list_matches() {
        let list = make_list_value(vec![
            make_string_value("user@gmail.com"),
            make_string_value("user@spotify.com"),
        ]);
        assert!(evaluate_criterion(&make_ends_with_criterion("@spotify.com"), &list).unwrap());
    }

    #[test]
    fn ends_with_no_value_in_list_matches() {
        let list = make_list_value(vec![
            make_string_value("user@gmail.com"),
            make_string_value("user@example.com"),
        ]);
        assert!(!evaluate_criterion(&make_ends_with_criterion("@spotify.com"), &list).unwrap());
    }

    #[test]
    fn unrecognized_rule_returns_error() {
        let criterion = criterion::AttributeCriterion {
            attribute_name: "attr".to_string(),
            rule: None,
        };
        let list = make_list_value(vec![make_string_value("hello")]);
        let result = evaluate_criterion(&criterion, &list);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ResolveError::UnrecognizedRule);
    }

    fn assert_bool(value: &targeting::value::Value, expected: bool) {
        match value {
            targeting::value::Value::BoolValue(b) => assert!(*b == expected),
            _ => assert!(false),
        }
    }

    fn assert_number(value: &targeting::value::Value, expected: f64) {
        match value {
            targeting::value::Value::NumberValue(n) => assert!(*n == expected),
            _ => assert!(false),
        }
    }

    fn assert_string(value: &targeting::value::Value, expected: &str) {
        match value {
            targeting::value::Value::StringValue(s) => assert!(*s == expected.to_string()),
            _ => assert!(false),
        }
    }

    fn assert_timestamp(value: &targeting::value::Value, expected: &chrono::DateTime<FixedOffset>) {
        match value {
            targeting::value::Value::TimestampValue(ts) => {
                assert!(ts.seconds == expected.timestamp());
                assert!(ts.nanos == expected.timestamp_subsec_nanos() as i32);
            }
            _ => assert!(false),
        }
    }

    fn assert_version(value: &targeting::value::Value, expected: &str) {
        match value {
            targeting::value::Value::VersionValue(v) => assert!(v.version == expected.to_string()),
            _ => assert!(false),
        }
    }
}
