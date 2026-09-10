use crate::{
    api,
    grpc::flag_resolver_service_server::FlagResolverService,
    service::{ResolverService, ServiceError},
};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use std::sync::Arc;

pub fn router(service: Arc<ResolverService>) -> Router {
    Router::new()
        .route("/v1/flags:resolve", post(resolve))
        .route("/v1/flags:apply", post(apply))
        .route("/v1/health", get(health).post(health))
        .route("/v1/metrics", get(metrics))
        .route("/v1/telemetry", get(metrics))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(service)
}
async fn resolve(State(service): State<Arc<ResolverService>>, body: Bytes) -> Response {
    match serde_json::from_slice::<api::ResolveFlagsRequest>(&body) {
        Ok(request) => match service.resolve(request).await {
            Ok(response) => Json(response).into_response(),
            Err(e) => http_error(e),
        },
        Err(_) => (StatusCode::BAD_REQUEST, "Invalid resolve request JSON").into_response(),
    }
}
async fn apply(State(service): State<Arc<ResolverService>>, body: Bytes) -> Response {
    match serde_json::from_slice::<api::ApplyFlagsRequest>(&body) {
        Ok(request) => match service.apply(request).await {
            Ok(response) => Json(response).into_response(),
            Err(e) => http_error(e),
        },
        Err(_) => (StatusCode::BAD_REQUEST, "Invalid apply request JSON").into_response(),
    }
}
async fn health(State(service): State<Arc<ResolverService>>) -> StatusCode {
    if service.ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}
async fn metrics(State(service): State<Arc<ResolverService>>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        service.metrics(),
    )
}
fn http_error(error: ServiceError) -> Response {
    // Match the existing servlet's resolve/apply error convention.
    (StatusCode::BAD_REQUEST, error.to_string()).into_response()
}
fn grpc_error(error: ServiceError) -> tonic::Status {
    match error {
        ServiceError::Unavailable | ServiceError::Materializations => {
            tonic::Status::unavailable(error.to_string())
        }
        ServiceError::Unauthenticated => tonic::Status::unauthenticated(error.to_string()),
        ServiceError::Invalid(message) => tonic::Status::invalid_argument(message),
    }
}
#[derive(Clone)]
pub struct GrpcResolver(pub Arc<ResolverService>);
#[tonic::async_trait]
impl FlagResolverService for GrpcResolver {
    async fn resolve_flags(
        &self,
        request: tonic::Request<api::ResolveFlagsRequest>,
    ) -> Result<tonic::Response<api::ResolveFlagsResponse>, tonic::Status> {
        self.0
            .resolve(request.into_inner())
            .await
            .map(tonic::Response::new)
            .map_err(grpc_error)
    }
    async fn apply_flags(
        &self,
        request: tonic::Request<api::ApplyFlagsRequest>,
    ) -> Result<tonic::Response<api::ApplyFlagsResponse>, tonic::Status> {
        self.0
            .apply(request.into_inner())
            .await
            .map(tonic::Response::new)
            .map_err(grpc_error)
    }
}
