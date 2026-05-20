//! Local HTTP server that serves per-unit `ResolverState` slices on demand.
//!
//! Intended for the unit-local resolver experiment described in
//! `UNIT_LOCAL_RESOLVER.md`. Loads the checked-in `resolver_state.pb`
//! fixture at startup and slices it for each request.
//!
//! Endpoints:
//!
//! - `GET /v1/account-config` → JSON describing the served account, the
//!   current `state_file_hash`, and the randomization unit fields the
//!   client should populate at resolve time.
//! - `GET /v1/resolver-state/:state_file_hash/:unit` → protobuf-encoded
//!   sliced `ResolverState`, with `X-Randomization-Unit-Fields` and
//!   `X-State-File-Hash` headers.
//! - `POST /v1/apply` → accepts a protobuf-encoded `ApplyFlagsRequest`
//!   and logs the flags + resolve token. The prototype does not forward
//!   to a real apply pipeline; productionising unit-local mode would
//!   plumb this through to the existing apply backend.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use confidence_resolver::proto::confidence::flags::admin::v1::ResolverState as ResolverStatePb;
use confidence_resolver::proto::confidence::flags::resolver::v1::ApplyFlagsRequest;
use confidence_resolver::proto::Message;
use confidence_resolver::slicer;
use serde::Serialize;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

const ACCOUNT_ID: &str = "confidence-demo-june";
const STATE_FILE_HASH: &str = "v1";
const EXAMPLE_STATE: &[u8] =
    include_bytes!("../../confidence-resolver/test-payloads/resolver_state.pb");

struct AppState {
    full_state: ResolverStatePb,
    full_state_size: usize,
    randomization_unit_fields: Vec<String>,
}

#[derive(Serialize)]
struct AccountConfig {
    account_id: String,
    state_file_hash: String,
    randomization_unit_fields: Vec<String>,
    full_state_size_bytes: usize,
}

fn collect_selector_fields(state: &ResolverStatePb) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for flag in &state.flags {
        for rule in &flag.rules {
            let selector = if rule.targeting_key_selector.is_empty() {
                "targeting_key".to_string()
            } else {
                rule.targeting_key_selector.clone()
            };
            seen.insert(selector);
        }
    }
    seen.into_iter().collect()
}

async fn account_config(State(state): State<Arc<AppState>>) -> Json<AccountConfig> {
    Json(AccountConfig {
        account_id: ACCOUNT_ID.to_string(),
        state_file_hash: STATE_FILE_HASH.to_string(),
        randomization_unit_fields: state.randomization_unit_fields.clone(),
        full_state_size_bytes: state.full_state_size,
    })
}

async fn resolver_state(
    State(state): State<Arc<AppState>>,
    Path((state_hash, unit)): Path<(String, String)>,
) -> Response {
    if state_hash != STATE_FILE_HASH {
        return (
            StatusCode::NOT_FOUND,
            format!("unknown state hash {state_hash}"),
        )
            .into_response();
    }

    let sliced = match slicer::slice_for_unit(state.full_state.clone(), ACCOUNT_ID, &unit) {
        Ok(s) => s,
        Err(e) => {
            error!("slice failed for unit={unit}: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "slice failed").into_response();
        }
    };

    let bytes = sliced.encode_to_vec();
    let sliced_len = bytes.len();
    info!(
        unit = %unit,
        sliced_bytes = sliced_len,
        full_bytes = state.full_state_size,
        ratio_pct = format!(
            "{:.2}",
            sliced_len as f64 / state.full_state_size as f64 * 100.0
        ),
        "served slice"
    );

    (
        [
            (header::CONTENT_TYPE, "application/x-protobuf".to_string()),
            (
                header::HeaderName::from_static("x-randomization-unit-fields"),
                state.randomization_unit_fields.join(","),
            ),
            (
                header::HeaderName::from_static("x-state-file-hash"),
                STATE_FILE_HASH.to_string(),
            ),
        ],
        bytes,
    )
        .into_response()
}

async fn apply_flags(body: Bytes) -> Response {
    match ApplyFlagsRequest::decode(body.as_ref()) {
        Ok(req) => {
            let flag_names: Vec<String> = req
                .flags
                .iter()
                .map(|f| f.flag.clone())
                .collect();
            info!(
                resolve_token_len = req.resolve_token.len(),
                client_secret_present = !req.client_secret.is_empty(),
                flags = ?flag_names,
                "received apply"
            );
            StatusCode::OK.into_response()
        }
        Err(err) => {
            error!("failed to decode ApplyFlagsRequest: {err:?}");
            (StatusCode::BAD_REQUEST, "decode failed").into_response()
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let full_state = ResolverStatePb::decode(EXAMPLE_STATE)?;
    let randomization_unit_fields = collect_selector_fields(&full_state);
    let app_state = Arc::new(AppState {
        full_state_size: EXAMPLE_STATE.len(),
        full_state,
        randomization_unit_fields,
    });

    info!(
        account_id = ACCOUNT_ID,
        state_file_hash = STATE_FILE_HASH,
        full_state_bytes = app_state.full_state_size,
        randomization_unit_fields = ?app_state.randomization_unit_fields,
        "loaded fixture"
    );

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/account-config", get(account_config))
        .route("/v1/resolver-state/:state_hash/:unit", get(resolver_state))
        .route("/v1/apply", post(apply_flags))
        .with_state(app_state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
