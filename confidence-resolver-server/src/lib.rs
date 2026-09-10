//! Native HTTP/gRPC harness for the Confidence resolver.
pub mod backend;
pub mod config;
pub mod reporting;
pub mod sampling;
pub mod service;
pub mod state;
pub mod transport;

#[allow(clippy::doc_lazy_continuation)]
pub mod grpc {
    tonic::include_proto!("confidence.flags.resolver.v1");
    pub const DESCRIPTOR: &[u8] = tonic::include_file_descriptor_set!("resolver_descriptor");
}

pub use confidence_resolver::proto::confidence::flags::resolver::v1 as api;
pub use service::ResolverService;

#[allow(dead_code)]
pub(crate) mod remote_proto {
    include!(concat!(
        env!("OUT_DIR"),
        "/remote/confidence.flags.resolver.v1.rs"
    ));
}
