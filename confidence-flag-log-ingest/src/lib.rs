use serde_json::Value;
use worker::*;

#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    if req.method() == Method::Options {
        return Response::ok("");
    }

    let router = Router::new();

    router
        .post_async("/v1/flagLogs:ingest", |mut req, ctx| async move {
            let account_id = match req.headers().get("X-Confidence-Account").ok().flatten() {
                Some(id) => id,
                None => return Response::error("Missing X-Confidence-Account header", 400),
            };

            let body_bytes: Vec<u8> = req.bytes().await?;

            let logs: Value = match serde_json::from_slice(&body_bytes) {
                Ok(v) => v,
                Err(e) => {
                    return Response::error(format!("Invalid JSON: {}", e), 400);
                }
            };

            let bucket = ctx.env.bucket("FLAG_LOGS_BUCKET")?;
            let timestamp_ms = js_sys::Date::new_0().get_time() as u64;
            let request_id = uuid::Uuid::new_v4();

            let key = format!(
                "{}_{}_{}.ndjson",
                account_id, timestamp_ms, request_id
            );

            let mut lines = Vec::new();

            if let Some(assignments) = logs.get("flag_assigned").and_then(|v| v.as_array()) {
                for assignment in assignments {
                    let mut line = assignment.clone();
                    if let Some(obj) = line.as_object_mut() {
                        obj.insert("type".to_string(), Value::String("flag_assigned".to_string()));
                    }
                    if let Ok(s) = serde_json::to_string(&line) {
                        lines.push(s);
                    }
                }
            }

            if let Some(telemetry) = logs.get("telemetry_data") {
                let mut line = telemetry.clone();
                if let Some(obj) = line.as_object_mut() {
                    obj.insert("type".to_string(), Value::String("telemetry".to_string()));
                }
                if let Ok(s) = serde_json::to_string(&line) {
                    lines.push(s);
                }
            }

            if let Some(flag_resolve_info) = logs.get("flag_resolve_info").and_then(|v| v.as_array()) {
                for info in flag_resolve_info {
                    let mut line = info.clone();
                    if let Some(obj) = line.as_object_mut() {
                        obj.insert("type".to_string(), Value::String("flag_resolve_info".to_string()));
                    }
                    if let Ok(s) = serde_json::to_string(&line) {
                        lines.push(s);
                    }
                }
            }

            if let Some(client_resolve_info) = logs.get("client_resolve_info").and_then(|v| v.as_array()) {
                for info in client_resolve_info {
                    let mut line = info.clone();
                    if let Some(obj) = line.as_object_mut() {
                        obj.insert("type".to_string(), Value::String("client_resolve_info".to_string()));
                    }
                    if let Ok(s) = serde_json::to_string(&line) {
                        lines.push(s);
                    }
                }
            }

            if lines.is_empty() {
                return Response::ok("{}");
            }

            let ndjson = lines.join("\n");

            bucket
                .put(&key, ndjson)
                .http_metadata(HttpMetadata {
                    content_type: Some("application/x-ndjson".to_string()),
                    ..Default::default()
                })
                .execute()
                .await?;

            Response::ok("{}")
        })
        .run(req, env)
        .await
}
