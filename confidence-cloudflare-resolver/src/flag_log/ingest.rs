//! Receives Logpush batches over HTTP and delivers them to the configured
//! log destinations.
//!
//! Logpush POSTs a gzipped newline-delimited batch of trace events to this
//! route; the handler pulls the flag logs out, deduplicates applies,
//! aggregates, and delivers. It is the whole consumer side — there is no R2,
//! no queue and no cron.
//!
//! # Delivery is best effort
//!
//! A batch that cannot be delivered is **dropped**, loudly. That is a
//! deliberate choice rather than an oversight: the alternative is returning
//! an error so Logpush retries, and Logpush responds to sustained failure by
//! *disabling the job*, which would silently stop the entire pipeline until
//! someone noticed. Dropping a batch loses that batch; a disabled job loses
//! everything until it is found. Always answering `200` keeps the pipeline
//! running and makes the loss visible in the logs instead.
//!
//! # Feedback
//!
//! This route runs on the same Worker whose trace events Logpush captures,
//! so handling a batch produces a trace event of its own. That is bounded
//! and tiny: those events carry no flag-log line, so they aggregate to
//! nothing, and at any real traffic level a handful of handler invocations
//! is lost among the thousands of resolves in each batch.
use super::logpush;
use flate2::read::MultiGzDecoder;
use std::io::Read;
use worker::{console_log, Env, Request, Response, Result};

/// Worker secret Logpush authenticates with, passed by the job as a
/// `header_Authorization` parameter on the destination URL.
const INGEST_TOKEN_VAR: &str = "FLAG_LOGS_INGEST_TOKEN";

/// Refuse a body larger than this once decompressed.
///
/// Logpush's batch floor is several MB and the handler holds one batch
/// decoded, so this bounds the invocation well inside the isolate's 128 MB
/// while leaving room for a batch to grow.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Handles one Logpush batch.
///
/// Always answers `200` once authenticated — see the note above on why a
/// failure is dropped rather than retried.
pub(crate) async fn handle(mut req: Request, env: &Env) -> Result<Response> {
    if !authorized(&req, env) {
        // 401 rather than a silent accept: a misconfigured job should fail
        // at setup, not appear to work while discarding everything.
        return Response::error("Unauthorized", 401);
    }

    let body = req.bytes().await?;

    // Logpush validates a destination by POSTing a gzipped `{"content":"tests"}`
    // before the job is created. It carries no trace events, so it falls out
    // of the parsing below naturally — but it must be answered 200 or the
    // job cannot be created at all.
    let Some(text) = decompress(&body) else {
        console_log!(
            "flag log ingest: body of {} bytes is not readable gzip or UTF-8; dropping",
            body.len()
        );
        return Response::ok("");
    };

    let extracted = logpush::extract(&text);
    let mut logs = extracted.logs;
    let skipped = extracted.skipped;
    if logs.is_empty() {
        if skipped > 0 {
            console_log!("flag log ingest: {} unparseable records, no logs", skipped);
        }
        return Response::ok("");
    }

    // Every record in a batch comes from the same deployment, so the first
    // destination speaks for all of them.
    let Some(destination) = extracted.destination else {
        console_log!("flag log ingest: no destination in batch; dropping");
        return Response::ok("");
    };
    let records = logs.len();
    // Deduplicated across the whole batch. This is the only place
    // cross-isolate duplicates can be caught: a batch carries records from
    // many isolates, while the resolver's own window only ever sees one.
    if super::dedup_enabled(env) {
        super::dedup_batch_flag_applies(&mut logs, (js_sys::Date::now() / 1000.0) as i64);
    }

    let delivered = super::deliver_within_limit(destination, logs).await;
    console_log!(
        "flag log ingest: {} bytes, {} records, {} bad, delivered={}",
        text.len(),
        records,
        skipped,
        delivered
    );
    if !delivered {
        console_log!(
            "flag log ingest: DROPPED {} records after failed delivery",
            records
        );
    }

    Response::ok("")
}

/// Constant-time-ish comparison is not required here: the token is a
/// deploy-time secret compared once per batch, not a per-request credential
/// under attacker-controlled timing pressure.
fn authorized(req: &Request, env: &Env) -> bool {
    let Ok(expected) = env.secret(INGEST_TOKEN_VAR).map(|s| s.to_string()) else {
        console_log!(
            "flag log ingest: {} secret is missing; rejecting",
            INGEST_TOKEN_VAR
        );
        return false;
    };
    let presented = req
        .headers()
        .get("Authorization")
        .ok()
        .flatten()
        .unwrap_or_default();
    presented == format!("Bearer {expected}")
}

/// Logpush always gzips the body. Plain bytes pass through so a manual probe
/// still works.
fn decompress(bytes: &[u8]) -> Option<String> {
    let limit = MAX_BODY_BYTES as u64 + 1;
    let mut text = String::new();
    let read = if bytes.starts_with(&[0x1f, 0x8b]) {
        MultiGzDecoder::new(bytes)
            .take(limit)
            .read_to_string(&mut text)
    } else {
        bytes.take(limit).read_to_string(&mut text)
    };
    read.ok()?;
    if text.len() > MAX_BODY_BYTES {
        // Reported by the caller: `console_log!` aborts off wasm32 and this
        // is the part worth covering with native tests.
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn decompresses_a_gzipped_batch() {
        let text = "{\"Logs\":[]}\n{\"Logs\":[]}\n";
        assert_eq!(decompress(&gzip(text)).as_deref(), Some(text));
    }

    /// Logpush POSTs exactly this to validate a destination before creating
    /// the job. It must decode, or the job cannot be created at all.
    #[test]
    fn accepts_the_logpush_validation_payload() {
        let probe = r#"{"content":"tests"}"#;
        assert_eq!(decompress(&gzip(probe)).as_deref(), Some(probe));
        // It carries no trace events, so it yields no logs rather than an error.
        let e = logpush::extract(probe);
        assert!(e.logs.is_empty());
        assert_eq!(e.skipped, 0);
    }

    #[test]
    fn passes_through_uncompressed_bodies() {
        assert_eq!(
            decompress(b"{\"Logs\":[]}").as_deref(),
            Some("{\"Logs\":[]}")
        );
    }

    #[test]
    fn rejects_truncated_gzip() {
        let bytes = gzip("{\"Logs\":[]}\n");
        assert_eq!(decompress(&bytes[..bytes.len() / 2]), None);
    }

    /// A compressible body must be refused without materialising it, which
    /// is why the cap is applied to the reader rather than to the output.
    #[test]
    fn rejects_a_body_that_decompresses_past_the_cap() {
        let bomb = gzip(&"a".repeat(MAX_BODY_BYTES + 1024));
        assert!(bomb.len() < 200_000, "bomb should be small compressed");
        assert_eq!(decompress(&bomb), None);
    }
}
