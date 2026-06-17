# Pre-baked Rust base image with system deps + toolchain from rust-toolchain.toml.
# Built separately so the main Dockerfile skips the ~21s install on cold cache.
FROM alpine:3.22

RUN apk add --no-cache \
    protobuf-dev \
    protoc \
    musl-dev \
    make \
    gcc \
    curl \
    ca-certificates

ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain none

WORKDIR /workspace

COPY rust-toolchain.toml ./

RUN rustup show
