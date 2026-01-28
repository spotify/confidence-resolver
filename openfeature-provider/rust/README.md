# Confidence OpenFeature Provider for Rust

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using the native Rust resolver
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging and resolve analytics
- **OpenFeature Compatible**: Works with the standard OpenFeature Rust SDK
- **Async/Await**: Built on Tokio for efficient async operations

## Requirements

- Rust 1.70+
- Tokio runtime
- OpenFeature Rust SDK 0.2.7+

## Installation

Add these dependencies to your `Cargo.toml`:

<!-- x-release-please-start-version -->
```toml
[dependencies]
spotify-confidence-openfeature-provider-local = "0.1.0"
open-feature = "0.2.7"
```
<!-- x-release-please-end -->

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Quick Start

```rust
use open_feature::{EvaluationContext, OpenFeature};
use spotify_confidence_openfeature_provider_local::{ConfidenceProvider, ProviderOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create provider options with your client secret
    let options = ProviderOptions::new("your-client-secret"); // Get from Confidence dashboard

    // Create the Confidence provider
    let provider = ConfidenceProvider::new(options)?;

    // Set the provider on the OpenFeature singleton
    OpenFeature::singleton_mut()
        .await
        .set_provider(provider)
        .await;

    // Create an OpenFeature client
    let client = OpenFeature::singleton().await.create_client();

    // Create evaluation context with user attributes for targeting
    let context = EvaluationContext::default()
        .with_targeting_key("user-123")
        .with_custom_field("country", "US")
        .with_custom_field("plan", "premium");

    // Evaluate a boolean flag
    let enabled = client
        .get_bool_value("test-flag.enabled", Some(&context), None)
        .await
        .unwrap_or(false);

    println!("Flag value: {}", enabled);

    Ok(())
}
```

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### Rust-Specific Examples

```rust
use open_feature::EvaluationContext;

// Simple attributes
let context = EvaluationContext::default()
    .with_targeting_key("user-123")
    .with_custom_field("country", "US")
    .with_custom_field("plan", "premium")
    .with_custom_field("age", 25);
```

## Error Handling

The provider uses a **default value fallback** pattern - when evaluation fails, it returns an error that you must handle with `.unwrap_or()` to apply your default value.

**See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:
- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### Rust-Specific Examples

```rust
// Using unwrap_or for default values
let enabled = client
    .get_bool_value("my-flag.enabled", Some(&context), None)
    .await
    .unwrap_or(false);
// enabled will be 'false' if evaluation failed

// For detailed error information, use get_bool_details()
let details = client
    .get_bool_details("my-flag.enabled", Some(&context), None)
    .await;

match details {
    Ok(result) => {
        println!("Value: {}", result.value);
        println!("Variant: {:?}", result.variant);
        println!("Reason: {:?}", result.reason);
    }
    Err(e) => {
        eprintln!("Flag evaluation error: {:?}", e);
    }
}
```

## Configuration

### ProviderOptions

The `ProviderOptions` struct contains all configuration options for the provider:

```rust
use spotify_confidence_openfeature_provider_local::ProviderOptions;

let options = ProviderOptions::new("your-client-secret")
    .with_initialize_timeout(10_000)      // Max ms to wait for initial state fetch
    .with_state_poll_interval(30_000)     // Interval in ms for polling state updates
    .with_confidence_materialization_store(); // Enable remote materialization
```

#### Required Fields

- `client_secret` (String): The client secret used for authentication and flag evaluation

#### Optional Fields

- `initialize_timeout_ms`: Max milliseconds to wait for initial state fetch (default: 30,000)
- `state_poll_interval_ms`: Interval in milliseconds for polling state updates (default: 30,000)
- `flush_interval_ms`: Interval in milliseconds for flushing logs (default: 10,000)
- `assign_flush_interval_ms`: Interval in milliseconds for flushing assign logs (default: 100)
- `materialization_store`: Storage for sticky variant assignments and materialized segments

## Flag Evaluation

The provider supports all OpenFeature value types:

```rust
// Boolean flags
let enabled = client
    .get_bool_value("feature.enabled", Some(&context), None)
    .await
    .unwrap_or(false);

// String flags
let color = client
    .get_string_value("feature.button_color", Some(&context), None)
    .await
    .unwrap_or_else(|_| "blue".to_string());

// Integer flags
let timeout = client
    .get_int_value("feature.timeout-ms", Some(&context), None)
    .await
    .unwrap_or(5000);

// Float flags
let ratio = client
    .get_float_value("feature.sampling_ratio", Some(&context), None)
    .await
    .unwrap_or(0.5);

// Object/structured flags
use open_feature::StructValue;
let config = client
    .get_struct_value::<StructValue>("feature", Some(&context), None)
    .await
    .unwrap_or_default();
```

## Logging

The provider uses the `tracing` crate for structured logging. Enable logging by initializing a tracing subscriber:

```rust
// Add to your Cargo.toml:
// tracing-subscriber = "0.3"

fn main() {
    // Initialize tracing with default settings
    tracing_subscriber::fmt::init();

    // Or with custom configuration
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();
}
```

The provider logs at different levels:
- `DEBUG`: Flag resolution details, state updates
- `INFO`: Provider initialization, configuration
- `WARN`: Non-critical issues, fallbacks
- `ERROR`: Failures, network errors

## Shutdown

**Important**: To ensure proper cleanup and flushing of exposure logs, you must explicitly shut down the provider rather than relying on the OpenFeature API shutdown function.

```rust
// Get the provider and shut it down explicitly
// Do NOT rely on OpenFeature::singleton_mut().await.shutdown().await
```

> **Why?** Due to an [upstream issue in the OpenFeature Rust SDK](https://github.com/open-feature/rust-sdk/issues/124), calling the OpenFeature shutdown may not properly wait for the provider shutdown to complete. This can result in loss of exposure logs and other telemetry data. Shutting down the provider directly ensures proper cleanup.

### What Happens During Shutdown?

1. **Flushes pending logs** to Confidence (exposure events, resolve analytics)
2. **Closes HTTP connections** and releases network resources
3. **Stops background tasks** (state polling, log batching)


## Materialization Stores

Materialization stores provide persistent storage for sticky variant assignments and custom targeting segments. This enables two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.

2. **Custom Targeting via Materialized Segments**: Precomputed sets of identifiers from datasets that should be targeted. Instead of evaluating complex targeting rules at runtime, materializations allow efficient lookup of whether a unit (user, session, etc.) is included in a target segment.

### Default Behavior

If your flags rely on sticky assignments or materialized segments, the default SDK behavior will prevent those rules from being applied and your evaluations will fall back to default values. For production workloads that need sticky behavior or segment lookups, configure a `MaterializationStore` to avoid unexpected fallbacks and ensure consistent variant assignment.

### Remote Materialization Store

For quick setup without managing your own storage infrastructure, enable the built-in remote materialization store:

```rust
let options = ProviderOptions::new("your-client-secret")
    .with_confidence_materialization_store();
```

**When to use**:
- You need sticky assignments or materialized segments but don't want to manage storage infrastructure
- Quick prototyping or getting started
- Lower-volume applications where network latency is acceptable

**Trade-offs**:
- Additional network calls during flag resolution (adds latency)
- Lower performance compared to local storage implementations (Redis, DynamoDB, etc.)

### Custom Implementations

For improved latency and reduced network calls, implement the `MaterializationStore` trait to store materialization data in your infrastructure:

```rust
use async_trait::async_trait;
use std::sync::Arc;
use spotify_confidence_openfeature_provider_local::{
    MaterializationStore, ReadOpType, ReadResultType, WriteOp,
    ProviderOptions,
};

struct MyRedisStore {
    // your implementation
}

#[async_trait]
impl MaterializationStore for MyRedisStore {
    async fn read_materializations(
        &self,
        read_ops: Vec<ReadOpType>,
    ) -> Result<Vec<ReadResultType>, spotify_confidence_openfeature_provider_local::Error> {
        // Load materialization data from Redis
        todo!()
    }

    async fn write_materializations(
        &self,
        write_ops: Vec<WriteOp>,
    ) -> Result<(), spotify_confidence_openfeature_provider_local::Error> {
        // Store materialization data to Redis
        todo!()
    }
}

// Use your custom store
let my_store = Arc::new(MyRedisStore { /* ... */ });
let options = ProviderOptions::new("your-client-secret")
    .with_materialization_store(my_store);
```

### When to Use Materialization Stores

Consider implementing a materialization store if:
- You need to support sticky variant assignments for experiments
- You use materialized segments for custom targeting
- You want to minimize network latency during flag resolution
- You have high-volume flag evaluations

If you don't use sticky assignments or materialized segments, the default behavior is sufficient.


## License

See the root `LICENSE` file.
