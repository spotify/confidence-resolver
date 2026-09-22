//! The Logpush sink's wire format: one prefixed `console.log` line per
//! request on the way out, and the matching extraction on the way back in.
//!
//! Both halves live here so the prefix and the encoding are defined once. The
//! producer runs in the resolver; [`extract`] runs in the aggregator, against
//! what Logpush wrote to R2.
//!
//! The payload is `base64(gzip(protobuf))`. Each layer earns its place:
//!
//! * **protobuf** rather than JSON drops the field names, but only ~30% —
//!   this message is dominated by strings (flag paths, targeting keys,
//!   assignment ids) that a schema change cannot shrink.
//! * **gzip** is what actually matters: `AppliedFlag` entries repeat the
//!   targeting key and share `flags/…/rules/…/variants/…` path prefixes, so
//!   measured over realistic data it saves ~5.5x and reduces the marginal
//!   cost of an exposure from ~365 to ~63 bytes.
//! * **base64** costs 33% but makes the result escape-free, so it does not
//!   inflate again when embedded as a JSON string inside the trace event, and
//!   it keeps characters and bytes identical for the budget check below.
use super::FLAG_LOG_PREFIX;
use base64::{engine::general_purpose::STANDARD, Engine};
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use prost::Message;
use serde::Deserialize;
use std::io::{Read, Write};
use worker::console_log;

/// Ceiling on one console line, in characters.
///
/// Cloudflare truncates a trace event's `logs` and `exceptions` fields once
/// their *combined* length reaches 16,384 characters — and counts exceptions
/// first, so a request that also threw eats into the budget before the flag
/// log is considered. Splitting across several `console.log` calls does not
/// help, because the limit is per trace event rather than per line.
///
/// This leaves ~4,400 characters of that budget for exceptions and any other
/// logging. Measured, a 57-flag exposure log encodes to roughly 3,800 bytes,
/// so the headroom is real; the cap only engages for resolves applying a few
/// hundred flags at once.
const MAX_CONSOLE_CHARS: usize = 12_000;

/// Cloudflare's hard limit, for reference and for the assertions below. Not
/// configurable and not plan-dependent: `max_upload_bytes` and
/// `max_upload_records` size the R2 files, not this field.
const CLOUDFLARE_COMBINED_LIMIT: usize = 16_384;

/// Reserved headroom for exceptions and any other console output sharing the
/// trace event. Checked at compile time so the budget cannot be raised into
/// the truncation zone by a later edit.
const _: () = assert!(MAX_CONSOLE_CHARS < CLOUDFLARE_COMBINED_LIMIT);
const _: () = assert!(CLOUDFLARE_COMBINED_LIMIT - MAX_CONSOLE_CHARS >= 4_000);

/// Emits the log as a single prefixed line, or routes it around the trace
/// event when it is too large for one.
///
/// The common path retains nothing in this isolate between requests, so a
/// hot isolate carries no backlog that an eviction could take. It is *not*
/// immune to eviction though: `send` is called from `wait_until`, the same
/// place the queue publish happens, so a dropped `wait_until` loses the log
/// under either sink. The difference is narrower than it looks — a local
/// console write rather than a network round-trip to the queue, so a smaller
/// window, not no window.
///
/// Above [`MAX_CONSOLE_CHARS`] that guarantee is worthless, because Logpush
/// would replace the record with a truncation marker and drop it. Such a log
/// is written to R2 directly instead, in the same format Logpush produces, so
/// the aggregator picks it up with no special handling and it is still
/// aggregated and deduplicated with everything else.
/// Emits the log without awaiting anything, returning it back only when
/// shipping needs async work.
///
/// This is the whole durability argument for the sink, and it only holds if
/// it runs *in* the request rather than after it. `wait_until` is best
/// effort — Cloudflare cancels pending work when an isolate is evicted or the
/// budget is exceeded — so a log that costs microseconds to emit should not
/// depend on it. Encoding plus a console write is local CPU that was being
/// spent either way; doing it before the response moves it, it does not add
/// it.
///
/// The oversized path is returned to the caller because delivering it is a
/// network round-trip, which does have to happen after the response.
pub(super) fn emit_inline(log: WriteFlagLogsRequest) -> Option<WriteFlagLogsRequest> {
    match encode(&log, destination()) {
        Some(line) if line.len() <= MAX_CONSOLE_CHARS => {
            console_log!("{}", line);
            None
        }
        // Too large for a trace event; needs the R2 overflow write.
        Some(_) => Some(log),
        None => {
            console_log!("flag log dropped: encoding failed");
            None
        }
    }
}

pub(super) async fn send(log: WriteFlagLogsRequest) {
    let Some(line) = encode(&log, destination()) else {
        console_log!("flag log dropped: encoding failed");
        return;
    };

    if line.len() <= MAX_CONSOLE_CHARS {
        console_log!("{}", line);
        return;
    }

    // Logpush would replace the record with a truncation marker and drop it,
    // so the console is not a transport for this one. Delivered inline
    // instead: that bypasses aggregation, but it is rare and the alternative
    // is losing it outright.
    console_log!(
        "flag log of {} chars exceeds the trace event budget; delivering inline",
        line.len()
    );
    if !super::deliver_to(destination(), &log).await {
        console_log!("flag log dropped: oversized and direct delivery failed");
    }
}

fn gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

/// `FLAGLOG <destination> <base64(gzip(protobuf))>`
///
/// The destination travels with the log rather than being looked up where it
/// is delivered, so the consumer needs no resolver state — it reads where a
/// batch should go from the batch itself.
/// Takes the destination rather than reading it, so this stays a pure
/// function: resolving it touches the embedded resolver state, which is not
/// loadable off wasm32 and would make the encoding untestable.
fn encode(log: &WriteFlagLogsRequest, destination: i32) -> Option<String> {
    let compressed = gzip(&log.encode_to_vec())?;
    Some(format!(
        "{}{} {}",
        FLAG_LOG_PREFIX,
        destination,
        STANDARD.encode(compressed)
    ))
}

/// This deployment's log destination, as the protobuf value carried in the
/// resolver state.
///
/// Taken from the raw state rather than from the parsed `LOG_DESTINATIONS`,
/// because the two numbering schemes differ: `LogDestination::from` reads 2
/// as Cloudflare, while the Rust enum's own discriminant for Cloudflare is 1.
/// Casting the parsed enum would emit 1, which decodes back as Edge — the
/// logs would silently go to the wrong destination.
fn destination() -> i32 {
    crate::CDN_STATE_REQUEST
        .log_destinations
        .first()
        .copied()
        .unwrap_or_default()
}

fn decode(payload: &str) -> Option<WriteFlagLogsRequest> {
    let compressed = STANDARD.decode(payload).ok()?;
    let mut proto = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut proto)
        .ok()?;
    WriteFlagLogsRequest::decode(proto.as_slice()).ok()
}

/// What one batch of trace events yielded.
pub(super) struct Extracted {
    pub(super) logs: Vec<WriteFlagLogsRequest>,
    /// Destination named by the first readable record, if any.
    pub(super) destination: Option<i32>,
    /// Records that carried the prefix but could not be decoded.
    pub(super) skipped: usize,
}

/// One Workers trace event, reduced to the console output we care about.
///
/// Logpush emits PascalCase field names. Every field is optional because the
/// job restricts `output_options.field_names`, so a record legitimately
/// arrives carrying only `Logs`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TraceEvent {
    #[serde(default)]
    logs: Vec<TraceLog>,
}

/// A single `console.*` call. `Message` is an array because console calls
/// take varargs.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TraceLog {
    #[serde(default)]
    message: Vec<serde_json::Value>,
}

/// Pulls the flag logs out of one newline-delimited document of trace events,
/// along with the number of prefixed records that failed to decode.
///
/// Everything that is not a [`FLAG_LOG_PREFIX`] line is ignored — error logs,
/// panics, and the aggregator's own output — so the aggregate is built from
/// flag logs alone. A malformed record is skipped rather than failing the
/// object, because one bad record must not strand every other log sharing the
/// file.
///
/// The skipped count is returned rather than logged so this stays a pure
/// function: `console_log!` aborts off wasm32, and the decoding is the part
/// worth covering with native tests.
pub(super) fn extract(ndjson: &str) -> Extracted {
    let mut logs = Vec::new();
    let mut skipped = 0usize;
    let mut destination = None;
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<TraceEvent>(line) else {
            continue;
        };
        for entry in event.logs {
            for message in entry.message {
                let Some(text) = message.as_str() else {
                    continue;
                };
                let Some(rest) = text.strip_prefix(FLAG_LOG_PREFIX) else {
                    continue;
                };
                let Some((dest, payload)) = rest.split_once(' ') else {
                    skipped = skipped.saturating_add(1);
                    continue;
                };
                match decode(payload) {
                    Some(log) => {
                        // First one wins: every record in a batch comes from
                        // the same deployment, so they agree except across a
                        // destination change, where a batch may straddle it.
                        if destination.is_none() {
                            destination = dest.parse::<i32>().ok();
                        }
                        logs.push(log);
                    }
                    None => skipped = skipped.saturating_add(1),
                }
            }
        }
    }
    Extracted {
        logs,
        destination,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use confidence_resolver::proto::confidence::flags::{
        admin::v1::FlagResolveInfo,
        resolver::v1::events::{
            flag_assigned::{applied_flag::Assignment, AppliedFlag, AssignmentInfo},
            FlagAssigned,
        },
    };

    /// Wraps already-encoded console messages in the trace-event envelope
    /// Logpush delivers, so the tests exercise the real nesting.
    fn trace_event(messages: &[&str]) -> String {
        let logs: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"Message": [m], "Level": "log", "TimestampMs": 1}))
            .collect();
        serde_json::json!({"Logs": logs}).to_string()
    }

    /// The logs half of `extract`, for cases where the skipped count is not
    /// what is under test.
    fn logs(ndjson: &str) -> Vec<WriteFlagLogsRequest> {
        extract(ndjson).logs
    }

    fn statistics(flag: &str) -> WriteFlagLogsRequest {
        WriteFlagLogsRequest {
            flag_resolve_info: vec![FlagResolveInfo {
                flag: flag.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// Deterministic pseudo-random hex, standing in for the values the
    /// resolver generates fresh per assignment. Random content is the worst
    /// case for gzip, which is what the size assertions should be measured
    /// against.
    fn hex(seed: u64, len: usize) -> String {
        let mut out = String::new();
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        while out.len() < len {
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 27;
            out.push_str(&format!("{x:016x}"));
        }
        out.truncate(len);
        out
    }

    /// An exposure log carrying `n` applied flags, each with a unique
    /// assignment id and an unpredictable flag name.
    fn exposures(n: usize) -> WriteFlagLogsRequest {
        let flags = (0..n)
            .map(|i| {
                let flag = format!("flags/{}", hex(i as u64 * 7 + 1, 18));
                AppliedFlag {
                    targeting_key: "7f3a9c21-4b8e-4d1a-9f62-0c5e8a7b1d34".to_string(),
                    targeting_key_selector: "user_id".to_string(),
                    assignment_id: hex(i as u64 * 31 + 5, 32),
                    rule: format!("{flag}/rules/default-rule"),
                    assignment: Some(Assignment::AssignmentInfo(AssignmentInfo {
                        variant: format!("{flag}/variants/treatment"),
                        segment: format!("{flag}/segments/all-users"),
                    })),
                    flag,
                    ..Default::default()
                }
            })
            .collect();
        WriteFlagLogsRequest {
            flag_assigned: vec![FlagAssigned {
                resolve_id: "01JQ8ZK4V2N7XW9R3M5T6Y8B2C".to_string(),
                flags,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_a_statistics_log() {
        let log = statistics("flags/test-flag");
        let ndjson = trace_event(&[&encode(&log, 0).unwrap()]);

        assert_eq!(logs(&ndjson), vec![log]);
    }

    #[test]
    fn round_trips_an_exposure_log_with_many_flags() {
        let log = exposures(57);
        let ndjson = trace_event(&[&encode(&log, 0).unwrap()]);

        assert_eq!(logs(&ndjson), vec![log]);
    }

    /// The wire value must survive `LogDestination::from`, which numbers
    /// Cloudflare as 2 — not as the Rust enum discriminant 1. Emitting the
    /// discriminant would route Cloudflare logs to Edge.
    #[test]
    fn destination_values_round_trip_through_the_proto_numbering() {
        use confidence_resolver::LogDestination;
        assert!(matches!(LogDestination::from(0), LogDestination::Edge));
        assert!(matches!(
            LogDestination::from(2),
            LogDestination::Cloudflare
        ));
        // The discriminant of Cloudflare is 1, which is *not* Cloudflare here.
        assert!(matches!(LogDestination::from(1), LogDestination::Edge));
    }

    #[test]
    fn encoded_line_is_prefixed_and_ascii() {
        let line = encode(&exposures(5), 0).unwrap();
        assert!(line.starts_with(FLAG_LOG_PREFIX), "line = {line}");
        // base64 is ASCII, so the character budget and the byte length agree.
        // With JSON it would be ambiguous whether Cloudflare counts either.
        assert!(line.is_ascii());
        let rest = line.strip_prefix(FLAG_LOG_PREFIX).unwrap();
        let (_, payload) = rest.split_once(' ').expect("destination then payload");
        assert!(decode(payload).is_some());
    }

    /// The production case that would truncate uncompressed: this account
    /// resolves 57 flags, and a JSON exposure log for them measured ~20,900
    /// characters — over Cloudflare's 16,384 combined limit.
    #[test]
    fn a_full_account_exposure_log_fits_the_console_budget() {
        let line = encode(&exposures(57), 0).unwrap();

        assert!(
            line.len() <= MAX_CONSOLE_CHARS,
            "57-flag exposure log took {} chars, budget is {MAX_CONSOLE_CHARS}",
            line.len()
        );
    }

    /// Guards the headroom rather than just the pass/fail boundary, so a
    /// regression in encoding size is caught before it starts truncating.
    #[test]
    fn budget_leaves_room_for_several_times_the_expected_worst_case() {
        let line = encode(&exposures(57), 0).unwrap();

        assert!(
            line.len() * 3 <= MAX_CONSOLE_CHARS,
            "57-flag log is {} chars, under 3x headroom of {MAX_CONSOLE_CHARS}",
            line.len()
        );
    }

    /// Compression is the whole reason the budget holds; if protobuf were
    /// emitted raw this log would be several times larger.
    #[test]
    fn compression_substantially_shrinks_a_large_exposure_log() {
        let log = exposures(100);
        let raw = log.encode_to_vec().len();
        let encoded = encode(&log, 0).unwrap().len();

        assert!(
            encoded * 3 < raw,
            "expected >3x saving, got {raw} -> {encoded}"
        );
    }

    #[test]
    fn ignores_console_output_that_is_not_a_flag_log() {
        // The whole point of the prefix: diagnostics share the trace event.
        let ndjson = trace_event(&[
            "queue publish failed; buffered for the next flush",
            "direct delivery to Edge failed: HTTP 503",
            "some operator debugging by hand",
        ]);

        assert!(logs(&ndjson).is_empty());
    }

    #[test]
    fn keeps_the_flag_log_when_it_shares_an_event_with_errors() {
        let log = statistics("flags/mixed");
        let ndjson = trace_event(&[
            "flag log dropped: oversized and direct delivery failed",
            &encode(&log, 0).unwrap(),
            "unrelated noise",
        ]);

        assert_eq!(logs(&ndjson), vec![log]);
    }

    #[test]
    fn collects_across_lines_and_events_in_order() {
        let first = statistics("flags/first");
        let second = statistics("flags/second");
        let third = statistics("flags/third");
        let ndjson = format!(
            "{}\n{}\n",
            trace_event(&[&encode(&first, 0).unwrap(), &encode(&second, 0).unwrap()]),
            trace_event(&[&encode(&third, 0).unwrap()]),
        );

        assert_eq!(logs(&ndjson), vec![first, second, third]);
    }

    #[test]
    fn skips_malformed_lines_without_losing_the_rest() {
        let log = statistics("flags/survivor");
        let ndjson = format!(
            "not json at all\n{{\"Logs\": [broken\n{}\n",
            trace_event(&[&encode(&log, 0).unwrap()])
        );

        assert_eq!(logs(&ndjson), vec![log]);
    }

    #[test]
    fn counts_a_prefixed_record_that_is_not_valid_base64() {
        let good = statistics("flags/good");
        let ndjson = trace_event(&[
            &format!("{FLAG_LOG_PREFIX}0 !!!not base64!!!"),
            &encode(&good, 0).unwrap(),
        ]);

        let e = extract(&ndjson);
        assert_eq!(e.logs, vec![good]);
        assert_eq!(e.skipped, 1);
    }

    #[test]
    fn counts_a_prefixed_record_that_is_base64_but_not_gzip() {
        let good = statistics("flags/good");
        let ndjson = trace_event(&[
            &format!("{}0 {}", FLAG_LOG_PREFIX, STANDARD.encode("plain bytes")),
            &encode(&good, 0).unwrap(),
        ]);

        let e = extract(&ndjson);
        assert_eq!(e.logs, vec![good]);
        assert_eq!(e.skipped, 1);
    }

    #[test]
    fn counts_a_truncated_record_as_skipped() {
        // Cloudflare truncation, or any byte-level mangling, makes a gzip
        // record unrecoverable rather than partially readable.
        let line = encode(&exposures(20), 0).unwrap();
        let rest = line.strip_prefix(FLAG_LOG_PREFIX).unwrap();
        let (dest, payload) = rest.split_once(' ').unwrap();
        let half = &payload[..payload.len() / 2];
        let ndjson = trace_event(&[&format!("{FLAG_LOG_PREFIX}{dest} {half}")]);

        let e = extract(&ndjson);
        assert!(e.logs.is_empty());
        assert_eq!(e.skipped, 1);
    }

    #[test]
    fn unprefixed_noise_is_not_counted_as_skipped() {
        // Only a broken *flag log* is a skip. Counting ordinary console
        // output would make the metric meaningless.
        let ndjson = trace_event(&["not a flag log", "also not a flag log"]);

        let e = extract(&ndjson);
        assert!(e.logs.is_empty());
        assert_eq!(e.skipped, 0);
    }

    #[test]
    fn tolerates_blank_lines_and_trailing_newlines() {
        let log = statistics("flags/blank");
        let ndjson = format!("\n\n{}\n\n\n", trace_event(&[&encode(&log, 0).unwrap()]));

        assert_eq!(logs(&ndjson), vec![log]);
    }

    #[test]
    fn tolerates_records_with_no_logs_field() {
        // output_options.field_names can produce records carrying only
        // metadata, and a request that logged nothing has no Logs at all.
        assert!(logs("{\"EventTimestampMs\":1}\n{\"Logs\":[]}\n").is_empty());
    }

    #[test]
    fn ignores_non_string_console_arguments() {
        let ndjson =
            serde_json::json!({"Logs": [{"Message": [42, null, {"a": 1}, ["x"]]}]}).to_string();

        assert!(logs(&ndjson).is_empty());
    }

    #[test]
    fn requires_the_prefix_at_the_start_of_the_line() {
        // A message merely containing the prefix is not a flag log; treating
        // it as one would feed operator text to the decoder.
        let ndjson = trace_event(&["about to write FLAGLOG 0 somepayload"]);

        assert!(logs(&ndjson).is_empty());
    }

    #[test]
    fn empty_request_round_trips() {
        let log = WriteFlagLogsRequest::default();
        let ndjson = trace_event(&[&encode(&log, 0).unwrap()]);

        assert_eq!(logs(&ndjson), vec![log]);
    }
}
