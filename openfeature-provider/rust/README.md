# Confidence OpenFeature Provider for Rust

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using the native Rust resolver
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging and resolve analytics with automatic retry on transient failures
- **OpenFeature Compatible**: Works with the standard OpenFeature Rust SDK
- **Async/Await**: Built on Tokio for efficient async operations

## Requirements

- Rust 1.90+
- Tokio runtime
- OpenFeature Rust SDK 0.3.0+

## Installation

Add these dependencies to your `Cargo.toml`:

<!-- x-release-please-start-version -->
```toml
[dependencies]
spotify-confidence-openfeature-provider-local = "0.1.0"
open-feature = "0.3.0"
```
<!-- x-release-please-end -->

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Encryption

The provider fetches encrypted flag state and decrypts it when loading the resolver. Pass the encryption key for your client credential when creating the provider.

See the [Integration Guide](../INTEGRATION_GUIDE.md#encryption) for background and migration details.

Pass the encryption key via `ProviderOptions`:

```rust
let options = ProviderOptions::new("your-client-secret", "your-encryption-key"); // Get from Confidence Admin view
```

Each client credential has a unique encryption key, available alongside it in [Confidence Admin](https://app.confidence.spotify.com/admin/clients).

## Quick Start

```rust
use open_feature::{EvaluationContext, OpenFeature};
use spotify_confidence_openfeature_provider_local::{ConfidenceProvider, ProviderOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create provider options with your client secret
    let options = ProviderOptions::new("your-client-secret", "your-encryption-key"); // Get from Confidence dashboard

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

### Initialization and freshness (OpenFeature Rust 0.3)

`set_provider(...).await` and `set_named_provider(...).await` wait for one bounded
initial state fetch, but return `()` even when it fails. Registration completing
is **not a readiness guarantee**. The SDK has no standard lifecycle events or
`FATAL` status, and does not propagate initialization errors. Before state is
available, evaluations return `Err` with `ProviderNotReady`; consumers supply
their fallback with `unwrap_or`, as in the examples below.

Network failures, timeouts, HTTP 408/429/5xx, and HTTP 404 retry in the background.
404 remains retryable because state may still be provisioning. HTTP 401/403 and
other nonretryable HTTP responses, invalid encryption, decoding errors, and
rejected resolver state stop startup retries. The equivalent of a fatal startup
failure is `ProviderStatus::Error`, with the cause logged and returned by subsequent
evaluations. The provider's direct `init()` method also returns the terminal error;
the SDK's registration API cannot. Malformed encryption keys fail in `new()` before
any requests. Correct the credentials/configuration and register a new provider
after a terminal failure.

Once good state exists, **every** refresh failure preserves it and polling continues,
including authentication or malformed-state failures. `status()` reports `STALE`
after `max_state_age` since the last successful validation (a successful state load
or HTTP 304). Cached flags remain evaluable while stale. Only successful validation
restores `READY`. Status derives age directly from a monotonic clock, so a slow
request cannot delay stale reporting; no lifecycle event timer is needed in SDK 0.3.
The SDK does not expose provider status through its client or API; `status()` is
the provider trait accessor.

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
use std::time::Duration;
use spotify_confidence_openfeature_provider_local::ProviderOptions;

let options = ProviderOptions::new("your-client-secret", "your-encryption-key")
    .with_initialize_timeout(Duration::from_secs(10)) // Timeout for each state fetch
    .with_state_poll_interval(Duration::from_secs(30)) // Interval between state updates
    .with_confidence_materialization_store(); // Enable remote materialization
```

#### Required Fields

- `client_secret` (String): The client secret used for authentication and flag evaluation
- `encryption_key` (String): Encryption key for decrypting the flag state. Found in the [Confidence Admin view](https://app.confidence.spotify.com/admin/clients).

#### Optional Fields

- `initialize_timeout`: Timeout for each initial and background state fetch (default: 30 seconds)
- `state_poll_interval`: Interval between state updates after initialization (default: 30 seconds). Retryable startup failures are retried 1 second after each attempt until the provider is ready or encounters a terminal error.
- `max_state_age`: Positive duration since last successful state validation before reporting `STALE` (default: 5 minutes). Set with `with_max_state_age(Duration::from_secs(300))`. Cached evaluation continues regardless of age.
- `flush_interval`: Interval for flushing logs (default: 15 seconds)
- `assign_flush_interval`: Interval for flushing assign logs (default: 100 milliseconds)
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
- `DEBUG`: Flag resolution details, state updates, individual retry attempts
- `INFO`: Provider initialization, configuration
- `WARN`: Non-critical issues, fallbacks, log delivery failures after retries exhausted
- `ERROR`: Failures, non-retryable HTTP errors

Flag log delivery retries up to 3 times on transient failures (5xx, 408, 429) with exponential backoff (500ms base, 2x multiplier, ±10% jitter). Server `Retry-After` headers are respected when present.

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
let options = ProviderOptions::new("your-client-secret", "your-encryption-key")
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
let options = ProviderOptions::new("your-client-secret", "your-encryption-key")
    .with_materialization_store(my_store);
```

### When to Use Materialization Stores

Consider implementing a materialization store if:
- You need to support sticky variant assignments for experiments
- You use materialized segments for custom targeting
- You want to minimize network latency during flag resolution
- You have high-volume flag evaluations

If you don't use sticky assignments or materialized segments, the default behavior is sufficient.


## Advanced: Controlling Exposure Events

By default, every flag evaluation records an exposure event (apply). Only disable this for exceptional cases where this provider must not collect exposures at all.

For normal feature delivery and experiments, keep applies enabled. When exposure collection is disabled, Confidence does not receive assignment/exposure events for those evaluations. Experiment results, exposure counts, assignment diagnostics, and downstream reporting that depend on exposures can be incomplete or unavailable. Resolve analytics and telemetry are still sent, so this is not a general logging or privacy-off switch.

To disable exposure collection for **all** OpenFeature evaluations through this provider, call `with_disable_exposure_collection()` on the provider options:

```rust
let options = ProviderOptions::new("your-client-secret", "your-encryption-key").with_disable_exposure_collection();
```

To skip exposure collection for a single evaluation, pass `_confidence_skip_apply` in the evaluation context:

```rust
let context = EvaluationContext::default()
    .with_targeting_key("user-123")
    .with_custom_field("_confidence_skip_apply", true);

let value = client
    .get_bool_value("my-flag.enabled", Some(&context), None)
    .await;
```

The key is automatically stripped from the context before it reaches the resolver.

| Mechanism | Scope | Assignment/exposure events | Resolve logs and telemetry |
| --- | --- | --- | --- |
| `with_disable_exposure_collection()` provider option | All OpenFeature evaluations through this provider | Never queued; no deferred apply token is returned | Still sent |
| `_confidence_skip_apply` context key | One evaluation | No immediate exposure event for that evaluation | Still sent |

This is an advanced feature intended for exceptional cases. If you're considering using it, reach out to the Confidence team to discuss the best approach for your setup.

## License

See the root `LICENSE` file.

### Migrating to mandatory encryption

`encryption_key` is now required when constructing the provider. Supply exactly 64
hexadecimal characters. Missing, empty, or malformed keys fail before network
activity. The provider only fetches encrypted state and never falls back to plaintext.

Open [Confidence Admin → Clients](https://app.confidence.spotify.com/admin/clients),
select your client, and locate the credential used by the provider. Each credential
has its own unique encryption key, available alongside it. Use the key paired with
your configured client secret. Configure and verify encryption on your existing
SDK before upgrading.

Pass both credentials to `ProviderOptions::new(client_secret, encryption_key)`.
The `encryption_key` field is now a `String`, not an `Option<String>`.
