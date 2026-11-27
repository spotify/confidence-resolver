//! Structured console diagnostic for failed `flags:apply` requests.
//!
//! A failed apply only returns a 500 to the caller, and the Worker logged
//! nothing, so an operator looking at Worker logs could not tell that exposures
//! were being rejected, or why. This builds one fixed-shape JSON line per failure.
//!
//! The resolver's error strings may interpolate request data (flag names, for
//! example), so they are never logged as-is. Only a few exact shapes are
//! recognised and mapped to a fixed kind and fixed text; for those carrying an
//! `[XXXXXXXX]` code, the code is passed through only after strict validation.
//! The codes are derived from source locations: they help maintainers find
//! the failing call site, but they do not identify a cause and are not
//! mapped to one here.
//!
//! Building the line is pure and tested natively; emitting it needs the
//! Workers console, which aborts off wasm32, so that part lives in [`emit`].

use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    ResolverLookup,
    Apply,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::ResolverLookup => "resolver_lookup",
            Stage::Apply => "apply",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Classified {
    kind: &'static str,
    message: &'static str,
    diagnostic_code: Option<String>,
}

const TOKEN_NOT_V1_MSG: &str = "Resolve token decrypted but is not a V1 token. Possible causes \
     include an encryption key mismatch, an altered token, or an incompatible token format; \
     the cause is not determined.";
const TOKEN_DECRYPT_MSG: &str = "Resolve token could not be decrypted. Check that the configured \
     resolve token encryption key matches the one that issued the token and that the token \
     is passed through unaltered; key rotation is one possibility, not a confirmed cause.";
const INTERNAL_MSG: &str = "Apply failed with an internal resolver error. The diagnostic code \
     identifies the failing code location only; it does not by itself indicate a token or \
     key problem.";
const OTHER_APPLY_MSG: &str = "Apply was rejected by the resolver for a reason not classified \
     by this diagnostic.";
const RESOLVER_LOOKUP_MSG: &str =
    "Could not create a resolver for the request, so apply was not attempted.";

/// Returns the code if `s` is exactly 8 base64url characters.
fn parse_code(s: &str) -> Option<String> {
    let valid = s.len() == 8
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then(|| s.to_string())
}

/// Matches `{prefix}[XXXXXXXX]` with nothing before or after.
fn coded(msg: &str, prefix: &str) -> Option<String> {
    msg.strip_prefix(prefix)?
        .strip_prefix('[')?
        .strip_suffix(']')
        .and_then(parse_code)
}

fn classify(stage: Stage, msg: &str) -> Classified {
    let (kind, message, diagnostic_code) = match stage {
        Stage::ResolverLookup => ("resolver_lookup_failed", RESOLVER_LOOKUP_MSG, None),
        Stage::Apply => {
            if msg == "resolve token is not a V1 token" {
                ("token_not_v1", TOKEN_NOT_V1_MSG, None)
            } else if let Some(code) = coded(msg, "failed to decrypt resolve token ") {
                ("token_decrypt_failed", TOKEN_DECRYPT_MSG, Some(code))
            } else if let Some(code) = coded(msg, "internal error ") {
                ("internal", INTERNAL_MSG, Some(code))
            } else {
                ("other_apply_error", OTHER_APPLY_MSG, None)
            }
        }
    };
    Classified {
        kind,
        message,
        diagnostic_code,
    }
}

/// Builds the JSON line for a failed apply. `error` is only classified, never
/// copied into the output (apart from a validated diagnostic code).
pub fn format(stage: Stage, error: &str, http_status: u16) -> String {
    let c = classify(stage, error);
    let mut v = json!({
        "event": "flag_apply_failed",
        "operation": "flags:apply",
        "stage": stage.as_str(),
        "error_kind": c.kind,
        "http_status": http_status,
        "message": c.message,
        "worker_version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(code) = c.diagnostic_code {
        v["diagnostic_code"] = code.into();
    }
    v.to_string()
}

/// Writes one diagnostic line to the Worker console at error level.
pub fn emit(stage: Stage, error: &str, http_status: u16) {
    worker::console_error!("{}", format(stage, error, http_status));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parsed(stage: Stage, err: &str) -> Value {
        serde_json::from_str(&format(stage, err, 500)).unwrap()
    }

    #[test]
    fn token_not_v1() {
        let v = parsed(Stage::Apply, "resolve token is not a V1 token");
        assert_eq!(v["event"], "flag_apply_failed");
        assert_eq!(v["operation"], "flags:apply");
        assert_eq!(v["stage"], "apply");
        assert_eq!(v["error_kind"], "token_not_v1");
        assert_eq!(v["http_status"], 500);
        assert_eq!(v["message"], TOKEN_NOT_V1_MSG);
        assert!(v.get("diagnostic_code").is_none());
    }

    #[test]
    fn token_decrypt_failed() {
        let v = parsed(Stage::Apply, "failed to decrypt resolve token [aZ09-_xQ]");
        assert_eq!(v["error_kind"], "token_decrypt_failed");
        assert_eq!(v["diagnostic_code"], "aZ09-_xQ");
    }

    #[test]
    fn internal_is_not_called_a_token_failure() {
        let v = parsed(Stage::Apply, "internal error [zKHkhc5y]");
        assert_eq!(v["error_kind"], "internal");
        assert_eq!(v["diagnostic_code"], "zKHkhc5y");
        let msg = v["message"].as_str().unwrap();
        assert!(!msg.contains("rotat"));
        assert!(msg.contains("does not by itself indicate a token"));
    }

    #[test]
    fn other_apply_error_is_generic() {
        let v = parsed(
            Stage::Apply,
            "Missing apply time for flag flags/secret-canary",
        );
        assert_eq!(v["error_kind"], "other_apply_error");
        assert_eq!(v["message"], OTHER_APPLY_MSG);
        assert!(v.get("diagnostic_code").is_none());
    }

    #[test]
    fn resolver_lookup_ignores_the_message() {
        for err in [
            "client secret not found",
            "internal error [zKHkhc5y]",
            "resolve token is not a V1 token",
        ] {
            let v = parsed(Stage::ResolverLookup, err);
            assert_eq!(v["stage"], "resolver_lookup");
            assert_eq!(v["error_kind"], "resolver_lookup_failed");
            assert!(v.get("diagnostic_code").is_none());
        }
    }

    #[test]
    fn malformed_or_suffixed_codes_are_rejected() {
        for err in [
            "internal error [zKHkhc5]",
            "internal error [zKHkhc5yy]",
            "internal error [zKHk+c5y]",
            "internal error [zKHk/c5y]",
            "internal error [zKHk=c5y]",
            "internal error [zKHk c5y]",
            "internal error [zKHkhc5y] flags/secret",
            "internal error [zKHkhc5y]\n",
            " internal error [zKHkhc5y]",
            "internal error zKHkhc5y",
            "internal error [zKHkhc5y",
            "internal error [ékHkhc5]",
            "failed to decrypt resolve token [abc]",
            "failed to decrypt resolve token [abcdefgh] extra",
            "resolve token is not a V1 token ",
            "Resolve token is not a V1 token",
        ] {
            let v = parsed(Stage::Apply, err);
            assert_eq!(v["error_kind"], "other_apply_error", "{err:?}");
            assert!(v.get("diagnostic_code").is_none(), "{err:?}");
        }
    }

    #[test]
    fn no_request_data_in_output() {
        let canaries = [
            "CANARY_CLIENT_SECRET",
            "flags/canary-flag",
            "CANARY_TOKEN_b64==",
            "canary-targeting-key",
        ];
        let errors = [
            "Missing apply time for flag flags/canary-flag".to_string(),
            format!(
                "Flag {} CANARY_TOKEN_b64== canary-targeting-key",
                canaries[1]
            ),
            "internal error [zKHkhc5y] CANARY_CLIENT_SECRET".to_string(),
            "CANARY_CLIENT_SECRET".to_string(),
        ];
        for stage in [Stage::Apply, Stage::ResolverLookup] {
            for err in &errors {
                let out = format(stage, err, 500);
                for c in canaries {
                    assert!(!out.contains(c), "{out}");
                }
            }
        }
    }

    #[test]
    fn output_has_only_the_fixed_fields() {
        let v = parsed(Stage::Apply, "internal error [zKHkhc5y]");
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "diagnostic_code",
                "error_kind",
                "event",
                "http_status",
                "message",
                "operation",
                "stage",
                "worker_version"
            ]
        );
    }
}
