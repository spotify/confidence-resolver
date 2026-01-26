# syntax=docker/dockerfile:1

# ==============================================================================
# Base image with Rust toolchain (Alpine - more reliable than Debian)
# ==============================================================================
FROM alpine:3.22 AS rust-base

# Install system dependencies
# - protoc/protobuf-dev: Required for prost-build (proto compilation in build.rs)
# - musl-dev: Required for linking Rust binaries on Alpine
RUN apk add --no-cache \
    protobuf-dev \
    protoc \
    musl-dev \
    make \
    gcc \
    curl \
    ca-certificates

# Install rustup into system-wide dirs so later stages can cache/copy them
ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

# Install rustup with no default toolchain; the toolchain file will drive installs
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain none

WORKDIR /workspace

# Copy rust-toolchain.toml and let rustup configure everything automatically
COPY rust-toolchain.toml ./

# Install toolchain from rust-toolchain.toml (components + targets)
RUN rustup show

# ==============================================================================
# Dependencies layer - cached separately from source code
# ==============================================================================
FROM rust-base AS rust-deps

# Copy only dependency manifests first for better caching
COPY Cargo.toml Cargo.lock ./
COPY confidence-resolver/Cargo.toml ./confidence-resolver/
COPY confidence-cloudflare-resolver/Cargo.toml ./confidence-cloudflare-resolver/
COPY wasm-msg/Cargo.toml ./wasm-msg/
COPY wasm/rust-guest/Cargo.toml ./wasm/rust-guest/
COPY openfeature-provider/java/Cargo.toml ./openfeature-provider/java/
COPY openfeature-provider/js/Cargo.toml ./openfeature-provider/js/
COPY openfeature-provider/go/Cargo.toml ./openfeature-provider/go/
COPY openfeature-provider/rust/Cargo.toml ./openfeature-provider/rust/
COPY openfeature-provider/python/Cargo.toml ./openfeature-provider/python/

# Copy proto files (needed by build.rs)
COPY confidence-resolver/protos ./confidence-resolver/protos/
COPY wasm-msg/proto ./wasm-msg/proto/
COPY wasm/proto ./wasm/proto/

# Copy build.rs files
COPY confidence-resolver/build.rs ./confidence-resolver/
COPY wasm-msg/build.rs ./wasm-msg/
COPY wasm/rust-guest/build.rs ./wasm/rust-guest/

# Create dummy source files to build dependencies
RUN mkdir -p confidence-resolver/src && \
    echo "pub fn dummy() {}" > confidence-resolver/src/lib.rs && \
    mkdir -p confidence-cloudflare-resolver/src && \
    echo "pub fn dummy() {}" > confidence-cloudflare-resolver/src/lib.rs && \
    mkdir -p wasm-msg/src && \
    echo "pub fn dummy() {}" > wasm-msg/src/lib.rs && \
    mkdir -p wasm/rust-guest/src && \
    echo "pub fn dummy() {}" > wasm/rust-guest/src/lib.rs && \
    mkdir -p openfeature-provider/rust/src && \
    echo "pub fn dummy() {}" > openfeature-provider/rust/src/lib.rs

# Build dependencies (this layer will be cached)
RUN cargo build -p confidence_resolver --release 

# Build test dependencies including dev-dependencies (this layer will be cached)
RUN cargo test -p confidence_resolver --lib --no-run --release 

# Build WASM dependencies (this layer will be cached)
RUN cargo build -p rust-guest --target wasm32-unknown-unknown --profile wasm

# Build confidence-cloudflare-resolver dependencies (this layer will be cached)
RUN RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo build -p confidence-cloudflare-resolver --target wasm32-unknown-unknown --release

# ==============================================================================
# Test & Lint Base - Copy source for testing/linting (native builds)
# ==============================================================================
FROM rust-base AS rust-test-base

# Copy the dependency cache from deps stage
COPY --from=rust-deps /usr/local/cargo /usr/local/cargo
COPY --from=rust-deps /workspace/target /workspace/target

# Copy all Rust source files (workspace requires all members present)
COPY Cargo.toml Cargo.lock ./
COPY confidence-resolver/ ./confidence-resolver/
COPY confidence-cloudflare-resolver/ ./confidence-cloudflare-resolver/
COPY wasm-msg/ ./wasm-msg/
COPY wasm/rust-guest/ ./wasm/rust-guest/
COPY wasm/proto/ ./wasm/proto/
COPY openfeature-provider/java/Cargo.toml ./openfeature-provider/java/
COPY openfeature-provider/js/Cargo.toml ./openfeature-provider/js/
COPY openfeature-provider/go/Cargo.toml ./openfeature-provider/go/
COPY openfeature-provider/rust/ ./openfeature-provider/rust/
COPY openfeature-provider/python/Cargo.toml ./openfeature-provider/python/

# Touch files to ensure rebuild (dependencies are cached)
RUN find . -type f -name "*.rs" -exec touch {} +
ENV IN_DOCKER_BUILD=1

# ==============================================================================
# Test confidence-resolver
# ==============================================================================
FROM rust-test-base AS confidence-resolver.test
WORKDIR /workspace/confidence-resolver
RUN make test

# ==============================================================================
# Test wasm-msg (when tests exist)
# ==============================================================================
FROM rust-test-base AS wasm-msg.test
WORKDIR /workspace/wasm-msg
RUN make test

# ==============================================================================
# Lint confidence-resolver
# ==============================================================================
FROM rust-test-base AS confidence-resolver.lint

WORKDIR /workspace/confidence-resolver
RUN make lint

# ==============================================================================
# Lint wasm-msg
# ==============================================================================
FROM rust-test-base AS wasm-msg.lint

WORKDIR /workspace/wasm-msg
RUN make lint

# ==============================================================================
# WASM Dependencies - Build WASM-specific dependencies
# ==============================================================================
FROM rust-base AS wasm-deps

# Copy the dependency cache from deps stage
COPY --from=rust-deps /usr/local/cargo /usr/local/cargo
COPY --from=rust-deps /workspace/target /workspace/target

# Copy only Rust-related source files (not Node.js/Python/Java)
COPY Cargo.toml Cargo.lock ./
COPY confidence-resolver/ ./confidence-resolver/
COPY confidence-cloudflare-resolver/ ./confidence-cloudflare-resolver/
COPY wasm-msg/ ./wasm-msg/
COPY wasm/rust-guest/ ./wasm/rust-guest/
COPY wasm/proto/ ./wasm/proto/
COPY openfeature-provider/java/Cargo.toml ./openfeature-provider/java/
COPY openfeature-provider/js/Cargo.toml ./openfeature-provider/js/
COPY openfeature-provider/go/Cargo.toml ./openfeature-provider/go/
COPY openfeature-provider/rust/ ./openfeature-provider/rust/
COPY openfeature-provider/python/Cargo.toml ./openfeature-provider/python/

# Copy data directory (needed by confidence-cloudflare-resolver include_str! macros)
COPY data/ ./data/

# Touch files to ensure rebuild
RUN find . -type f -name "*.rs" -exec touch {} +
ENV IN_DOCKER_BUILD=1

# ==============================================================================
# Build wasm/rust-guest WASM
# ==============================================================================
FROM wasm-deps AS wasm-rust-guest.build

WORKDIR /workspace/wasm/rust-guest
RUN make build

# Change back to workspace root to find the target directory
WORKDIR /workspace

# Verify build artifact
RUN ls -lh target/wasm32-unknown-unknown/wasm/rust_guest.wasm && \
    echo "WASM size: $(du -h target/wasm32-unknown-unknown/wasm/rust_guest.wasm | cut -f1)"

# ==============================================================================
# Lint wasm/rust-guest (WASM target)
# ==============================================================================
FROM wasm-deps AS wasm-rust-guest.lint

WORKDIR /workspace/wasm/rust-guest
RUN make lint

# ==============================================================================
# Extract wasm/rust-guest WASM artifact
# ==============================================================================
FROM scratch AS wasm-rust-guest.artifact

COPY --from=wasm-rust-guest.build /workspace/target/wasm32-unknown-unknown/wasm/rust_guest.wasm /confidence_resolver.wasm

# ==============================================================================
# Build confidence-cloudflare-resolver (WASM target)
# ==============================================================================
FROM wasm-deps AS confidence-cloudflare-resolver.build

WORKDIR /workspace/confidence-cloudflare-resolver
RUN make build

# ==============================================================================
# Lint confidence-cloudflare-resolver (WASM target)
# ==============================================================================
FROM wasm-deps AS confidence-cloudflare-resolver.lint

WORKDIR /workspace/confidence-cloudflare-resolver
RUN make lint

# ==============================================================================
# CloudFlare Deployer - Runtime image for deploying to CloudFlare Workers
# ==============================================================================
FROM confidence-cloudflare-resolver.build AS confidence-cloudflare-resolver.deployer

# Install Node.js, npm, jq, git, and bash for deployment
RUN apk add --no-cache nodejs npm jq git bash

# Install Wrangler CLI
RUN npm install -g wrangler@latest

# Install worker-build (Rust WASM build tool used by Wrangler)
ARG WORKER_BUILD_VERSION="0.1.11"
RUN cargo install worker-build --locked --version ${WORKER_BUILD_VERSION}

# Install wasm-bindgen-cli (required by worker-build, must match version in Cargo.lock)
RUN cargo install wasm-bindgen-cli --version 0.2.100

# Optionally pass the commit SHA at build time
ARG COMMIT_SHA=""
ENV COMMIT_SHA=${COMMIT_SHA}

# Return to workspace root
WORKDIR /workspace

# Clean sample/runtime data to avoid leaking into image
RUN rm -rf data/*

# Ensure deploy script is executable
RUN chmod +x confidence-cloudflare-resolver/deployer/script.sh

# Default command runs the deployer script
CMD ["./confidence-cloudflare-resolver/deployer/script.sh"]

# ==============================================================================
# Python Host - Run Python host example
# ==============================================================================
FROM python:3.11-slim AS python-host-base

# Install protobuf and dependencies (libprotobuf-dev includes google proto files)
RUN apt-get update && \
    apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev make && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy Makefile and proto generation script
COPY wasm/python-host/Makefile ./
COPY wasm/python-host/generate_proto.py ./

# Copy proto files (wasm proto + confidence-resolver protos needed for imports)
COPY wasm/proto ../proto/
COPY confidence-resolver/protos ../../confidence-resolver/protos/

# Build using Makefile (creates venv + installs deps + generates proto)
ENV IN_DOCKER_BUILD=1
RUN make build

# Copy source code
COPY wasm/python-host/*.py ./

# Copy WASM module
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm ../confidence_resolver.wasm

# Copy resolver state
COPY wasm/resolver_state.pb ../resolver_state.pb

# ==============================================================================
# Test Python Host (integration test)
# ==============================================================================
FROM python-host-base AS python-host.test
RUN make run

# ==============================================================================
# OpenFeature Provider (TypeScript) - Build and test
# ==============================================================================
FROM node:20-alpine AS openfeature-provider-js-base

# Install protoc for proto generation
RUN apk add --no-cache protobuf-dev protoc make

WORKDIR /app

# Enable Corepack for Yarn
RUN corepack enable

# Copy package files for dependency caching
COPY \
    openfeature-provider/js/Makefile \
    openfeature-provider/js/package.json \
    openfeature-provider/js/yarn.lock \
    openfeature-provider/js/.yarnrc.yml \
    openfeature-provider/js/README.md \
    openfeature-provider/js/CHANGELOG.md \
    openfeature-provider/js/LICENSE \
    ./
COPY openfeature-provider/js/proto ./proto

# Copy proto files needed for proto generation
COPY openfeature-provider/proto ../../../openfeature-provider/proto

# Install dependencies (this layer will be cached)
ENV IN_DOCKER_BUILD=1
RUN make install

# Copy source and config
COPY openfeature-provider/js/src ./src/
COPY openfeature-provider/js/tsconfig.json openfeature-provider/js/tsdown.config.ts openfeature-provider/js/vitest.config.ts ./
COPY openfeature-provider/js/Makefile ./

# Copy WASM module
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm ../../../wasm/confidence_resolver.wasm


# ==============================================================================
# Test OpenFeature Provider
# ==============================================================================
FROM openfeature-provider-js-base AS openfeature-provider-js.test

# Copy files needed for testing
COPY wasm/resolver_state.pb ../../../wasm/resolver_state.pb
COPY openfeature-provider/js/prettier.config.cjs ./
COPY openfeature-provider/js/.prettierignore ./

RUN make test

# ==============================================================================
# E2E Test OpenFeature Provider (requires credentials)
# ==============================================================================
FROM openfeature-provider-js.test AS openfeature-provider-js.test_e2e

# Run e2e tests with secrets mounted as .env.test file
RUN make test-e2e

# ==============================================================================
# Build OpenFeature Provider
# ==============================================================================
FROM openfeature-provider-js-base AS openfeature-provider-js.build

RUN make build

# Verify no bundle splitting occurred
RUN set -e; \
    echo "Verifying no bundle splitting in JS artifacts..."; \
    UNEXPECTED_FILES=$(find dist -name '*.js' ! -name 'index.node.js' ! -name 'index.inlined.js' ! -name 'index.fetch.js' | head -10); \
    if [ -n "$UNEXPECTED_FILES" ]; then \
      echo ""; \
      echo "❌ ERROR: Bundle splitting detected!"; \
      echo ""; \
      echo "Found unexpected JavaScript files in dist/:"; \
      echo "$UNEXPECTED_FILES"; \
      echo ""; \
      echo "Only index.node.js and index.browser.js should be present."; \
      echo "Check tsdown.config.ts configuration to prevent code splitting."; \
      echo ""; \
      exit 1; \
    fi; \
    echo "✅ No bundle splitting detected - only expected files present"

# ==============================================================================
# Pack OpenFeature Provider (JS) - Create tarball for publishing
# ==============================================================================
FROM openfeature-provider-js.build AS openfeature-provider-js.pack

RUN yarn pack

# ==============================================================================
# Extract OpenFeature Provider (JS) package artifact
# ==============================================================================
FROM scratch AS openfeature-provider-js.artifact

COPY --from=openfeature-provider-js.pack /app/package.tgz /package.tgz

# ==============================================================================
# OpenFeature Provider (Go) - Build and test
# ==============================================================================
FROM golang:1.24-alpine AS openfeature-provider-go-base

# Install make (needed for Makefile targets)
RUN apk add --no-cache make

WORKDIR /app

# Copy Makefile (at top level of go provider)
COPY openfeature-provider/go/Makefile ./

# Copy go.mod for dependency caching from confidence/ subdirectory
COPY openfeature-provider/go/go.mod openfeature-provider/go/go.sum ./

# Download Go dependencies (this layer will be cached)
RUN go mod download

# Copy source code
COPY openfeature-provider/go/confidence/ ./confidence/

# Set environment variable
ENV IN_DOCKER_BUILD=1

# ==============================================================================
# Validate WASM sync for Go Provider
# ==============================================================================
FROM alpine:3.22 AS openfeature-provider-go.validate-wasm

# Install diffutils for cmp command
RUN apk add --no-cache diffutils

# Copy built WASM from artifact
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm /built/confidence_resolver.wasm

# Copy committed WASM from source
COPY openfeature-provider/go/confidence/internal/local_resolver/assets/confidence_resolver.wasm /committed/confidence_resolver.wasm

# Compare files
RUN set -e; \
    echo "Validating WASM sync for Go provider..."; \
    if ! cmp -s /built/confidence_resolver.wasm /committed/confidence_resolver.wasm; then \
      echo ""; \
      echo "❌ ERROR: WASM files are out of sync!"; \
      echo ""; \
      echo "The WASM file in openfeature-provider/go/confidence/wasm/ doesn't match the built version."; \
      echo ""; \
      echo "To fix (using Docker to ensure correct dependencies):"; \
      echo "  docker build --target wasm-rust-guest.artifact --output type=local,dest=. ."; \
      echo "  cp confidence_resolver.wasm openfeature-provider/go/confidence/wasm/"; \
      echo "  git add openfeature-provider/go/confidence/wasm/confidence_resolver.wasm"; \
      echo "  git commit -m 'chore: sync WASM module for Go provider'"; \
      echo ""; \
      echo "Or use the Makefile target:"; \
      echo "  make sync-wasm-go"; \
      echo ""; \
      exit 1; \
    fi; \
    echo "✅ WASM files are in sync"

# ==============================================================================
# Test OpenFeature Provider (Go)
# ==============================================================================
FROM openfeature-provider-go-base AS openfeature-provider-go.test

RUN make test

# ==============================================================================
# Lint OpenFeature Provider (Go)
# ==============================================================================
FROM openfeature-provider-go-base AS openfeature-provider-go.lint

RUN make lint

# ==============================================================================
# Build OpenFeature Provider (Go)
# ==============================================================================
FROM openfeature-provider-go-base AS openfeature-provider-go.build

RUN make build

# ==============================================================================
# OpenFeature Provider (Ruby) - Build and test
# ==============================================================================
FROM ruby:3.3-alpine AS openfeature-provider-ruby-base

# Install build dependencies
RUN apk add --no-cache make git build-base openssl-dev

WORKDIR /app

# Copy Gemfile for dependency caching
COPY openfeature-provider/ruby/Gemfile ./
COPY openfeature-provider/ruby/confidence-openfeaure-provider.gemspec ./
COPY openfeature-provider/ruby/Makefile ./

# Copy lib directory (needed by gemspec to read version)
COPY openfeature-provider/ruby/lib ./lib/

# Install dependencies (this layer will be cached)
ENV IN_DOCKER_BUILD=1
RUN make install

# Copy remaining source code
COPY openfeature-provider/ruby/spec ./spec/
COPY openfeature-provider/ruby/Rakefile ./

# ==============================================================================
# Test OpenFeature Provider (Ruby)
# ==============================================================================
FROM openfeature-provider-ruby-base AS openfeature-provider-ruby.test

RUN make test

# ==============================================================================
# Lint OpenFeature Provider (Ruby)
# ==============================================================================
FROM openfeature-provider-ruby-base AS openfeature-provider-ruby.lint

RUN make lint

# ==============================================================================
# Build OpenFeature Provider (Ruby)
# ==============================================================================
FROM openfeature-provider-ruby-base AS openfeature-provider-ruby.build

RUN make build

# ==============================================================================
# Extract OpenFeature Provider (Ruby) gem artifact
# ==============================================================================
FROM scratch AS openfeature-provider-ruby.artifact

COPY --from=openfeature-provider-ruby.build /app/pkg/*.gem /

# ==============================================================================
# Publish OpenFeature Provider (Ruby) to RubyGems
# ==============================================================================
FROM openfeature-provider-ruby.build AS openfeature-provider-ruby.publish

RUN --mount=type=secret,id=rubygem_api_key \
    export GEM_HOST_API_KEY=$(cat /run/secrets/rubygem_api_key) && \
    gem push pkg/*.gem

# ==============================================================================
# OpenFeature Provider (Python) - Build and test
# ==============================================================================
FROM python:3.11-slim AS openfeature-provider-python-base

# Install protobuf and build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    libprotobuf-dev \
    make \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy Makefile and pyproject.toml for dependency caching
COPY openfeature-provider/python/Makefile ./
COPY openfeature-provider/python/pyproject.toml ./
COPY openfeature-provider/python/README.md ./
COPY openfeature-provider/python/.flake8 ./

# Copy proto files (needed for proto generation)
COPY openfeature-provider/proto ../proto/

# Copy source code
COPY openfeature-provider/python/src ./src/
COPY openfeature-provider/python/tests ./tests/

# Copy WASM module into resources
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm ./resources/wasm/confidence_resolver.wasm

# Copy test data fixtures (needed by tests)
# conftest.py expects data at ../../data relative to python dir
COPY data/ ../../data/

# Set environment variable
ENV IN_DOCKER_BUILD=1

# Install dependencies
RUN make install

# ==============================================================================
# Test OpenFeature Provider (Python)
# ==============================================================================
FROM openfeature-provider-python-base AS openfeature-provider-python.test

RUN make test

# ==============================================================================
# E2E Test OpenFeature Provider (Python) (requires credentials)
# ==============================================================================
FROM openfeature-provider-python.test AS openfeature-provider-python.test_e2e

RUN make test-e2e

# ==============================================================================
# Lint OpenFeature Provider (Python)
# ==============================================================================
FROM openfeature-provider-python-base AS openfeature-provider-python.lint

RUN make lint

# ==============================================================================
# Build OpenFeature Provider (Python)
# ==============================================================================
FROM openfeature-provider-python-base AS openfeature-provider-python.build

RUN make build

# ==============================================================================
# Extract OpenFeature Provider (Python) package artifact
# ==============================================================================
FROM scratch AS openfeature-provider-python.artifact

COPY --from=openfeature-provider-python.build /app/dist/*.whl /
COPY --from=openfeature-provider-python.build /app/dist/*.tar.gz /

# ==============================================================================
# OpenFeature Provider (Rust) - Test
# ==============================================================================
FROM rust-test-base AS openfeature-provider-rust.test

WORKDIR /workspace/openfeature-provider/rust
RUN make test

# ==============================================================================
# OpenFeature Provider (Rust) - E2E Test
# ==============================================================================
FROM rust-test-base AS openfeature-provider-rust.test_e2e

WORKDIR /workspace/openfeature-provider/rust
RUN make test-e2e

# ==============================================================================
# OpenFeature Provider (Rust) - Lint
# ==============================================================================
FROM rust-test-base AS openfeature-provider-rust.lint

WORKDIR /workspace/openfeature-provider/rust
RUN make lint

# ==============================================================================
# OpenFeature Provider (Rust) - Build
# ==============================================================================
FROM rust-test-base AS openfeature-provider-rust.build

WORKDIR /workspace/openfeature-provider/rust
RUN make build

# ==============================================================================
# OpenFeature Provider (Java) - Build and test
# ==============================================================================
FROM eclipse-temurin:17-jdk AS openfeature-provider-java-base

# Install Maven and protobuf (Debian-based for glibc compatibility)
RUN apt-get update && apt-get install -y \
    maven \
    protobuf-compiler \
    make \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy pom.xml for dependency caching
COPY openfeature-provider/java/pom.xml ./
COPY openfeature-provider/java/Makefile ./

# Download dependencies (this layer will be cached)
RUN mvn dependency:go-offline -q || true

# Copy proto files (needed for protobuf generation)
COPY openfeature-provider/proto ../proto/

# Copy source code
COPY openfeature-provider/java/src ./src/

# Copy WASM module into resources
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm ../../../wasm/confidence_resolver.wasm

# Set environment variable
ENV IN_DOCKER_BUILD=1

# ==============================================================================
# Test OpenFeature Provider (Java)
# ==============================================================================
FROM openfeature-provider-java-base AS openfeature-provider-java.test

RUN make test

# ==============================================================================
# E2E Test OpenFeature Provider (Java) (requires credentials)
# ==============================================================================
FROM openfeature-provider-java.test AS openfeature-provider-java.test_e2e

# Run e2e tests with secrets mounted as .env.test file
RUN make test-e2e

# ==============================================================================
# Build OpenFeature Provider (Java)
# ==============================================================================
FROM openfeature-provider-java-base AS openfeature-provider-java.build

RUN make build

# ==============================================================================
# Publish OpenFeature Provider (Java) to Maven Central
# ==============================================================================
FROM openfeature-provider-java.build AS openfeature-provider-java.publish

RUN --mount=type=secret,id=gpg_private_key \
    gpg --batch --import /run/secrets/gpg_private_key

RUN --mount=type=secret,id=maven_settings \
    --mount=type=secret,id=gpg_pass,env=MAVEN_GPG_PASSPHRASE \
    mvn -q -s /run/secrets/maven_settings --batch-mode -DskipTests deploy

# ==============================================================================
# All - Build and validate everything (default target)
# ==============================================================================
FROM scratch AS all

# Copy build artifacts (forces build stages to execute)
COPY --from=wasm-rust-guest.artifact /confidence_resolver.wasm /artifacts/wasm/

# Force test stages to run by copying marker files
COPY --from=confidence-resolver.test /workspace/Cargo.toml /markers/test-resolver
COPY --from=wasm-msg.test /workspace/Cargo.toml /markers/test-wasm-msg
COPY --from=openfeature-provider-js.test /app/package.json /markers/test-openfeature-js
COPY --from=openfeature-provider-js.test_e2e /app/package.json /markers/test-openfeature-js-e2e
COPY --from=openfeature-provider-java.test /app/pom.xml /markers/test-openfeature-java
COPY --from=openfeature-provider-java.test_e2e /app/pom.xml /markers/test-openfeature-java-e2e
COPY --from=openfeature-provider-go.test /app/go.mod /markers/test-openfeature-go
COPY --from=openfeature-provider-ruby.test /app/Gemfile /markers/test-openfeature-ruby
COPY --from=openfeature-provider-python.test /app/pyproject.toml /markers/test-openfeature-python
COPY --from=openfeature-provider-python.test_e2e /app/pyproject.toml /markers/test-openfeature-python-e2e
COPY --from=openfeature-provider-rust.test /workspace/Cargo.toml /markers/test-openfeature-rust
COPY --from=openfeature-provider-rust.test_e2e /workspace/Cargo.toml /markers/test-openfeature-rust-e2e

# Force validation stages to run
COPY --from=openfeature-provider-go.validate-wasm /built/confidence_resolver.wasm /markers/validate-wasm-go

# Force integration test stages to run (host examples)
COPY --from=python-host.test /app/Makefile /markers/integration-python

# Force lint stages to run by copying marker files
COPY --from=confidence-resolver.lint /workspace/Cargo.toml /markers/lint-resolver
COPY --from=wasm-msg.lint /workspace/Cargo.toml /markers/lint-wasm-msg
COPY --from=wasm-rust-guest.lint /workspace/Cargo.toml /markers/lint-guest
COPY --from=openfeature-provider-go.lint /app/go.mod /markers/lint-openfeature-go
COPY --from=openfeature-provider-ruby.lint /app/Gemfile /markers/lint-openfeature-ruby
COPY --from=openfeature-provider-python.lint /app/pyproject.toml /markers/lint-openfeature-python
COPY --from=openfeature-provider-rust.lint /workspace/Cargo.toml /markers/lint-openfeature-rust
COPY --from=confidence-cloudflare-resolver.lint /workspace/Cargo.toml /markers/lint-cloudflare

# Force build stages to run
COPY --from=confidence-cloudflare-resolver.build /workspace/Cargo.toml /markers/build-cloudflare
COPY --from=openfeature-provider-js.build /app/dist/index.node.js /artifacts/openfeature-js/
COPY --from=openfeature-provider-java.build /app/target/*.jar /artifacts/openfeature-java/
COPY --from=openfeature-provider-go.build /app/.build.stamp /artifacts/openfeature-go/
COPY --from=openfeature-provider-ruby.build /app/.build.stamp /artifacts/openfeature-ruby/
COPY --from=openfeature-provider-python.build /app/.build.stamp /artifacts/openfeature-python/
COPY --from=openfeature-provider-rust.build /workspace/openfeature-provider/rust/.build.stamp /artifacts/openfeature-rust/
