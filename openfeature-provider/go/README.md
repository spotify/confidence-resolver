# Confidence OpenFeature Provider for Go

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using WebAssembly (WASM)
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging and resolve analytics
- **OpenFeature Compatible**: Works with the standard OpenFeature Go SDK

## Installation

```bash
go get github.com/spotify/confidence-resolver/openfeature-provider/go
go mod tidy
```

## Requirements

- Go 1.24+
- OpenFeature Go SDK 1.16.0+

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:

- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Quick Start

```go
package main

import (
    "context"
    "log"

    "github.com/open-feature/go-sdk/openfeature"
    "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
)

func main() {
    ctx := context.Background()

    // Create provider with your client secret
    provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
        ClientSecret: "your-client-secret", // Get from Confidence dashboard
    })
    if err != nil {
        log.Fatalf("Failed to create provider: %v", err)
    }

    // Set the provider and wait for initialization
    openfeature.SetProviderAndWait(provider)

    // Get a client
    client := openfeature.NewClient("my-app")

    // Create evaluation context with user attributes for targeting
    evalCtx := openfeature.NewEvaluationContext("user-123", map[string]interface{}{
        "country": "US",
        "plan":    "premium",
    })

    // Evaluate a flag
    value, err := client.BooleanValue(ctx, "test-flag.enabled", false, evalCtx)
    if err != nil {
        log.Printf("Flag evaluation failed, using default: %v", err)
    }

    log.Printf("Flag value: %v", value)
}
```

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### Go-Specific Examples

```go
// Simple attributes
evalCtx := openfeature.NewEvaluationContext("user-123", map[string]interface{}{
    "country": "US",
    "plan":    "premium",
    "age":     25,
})
```

## Error Handling

The provider uses a **default value fallback** pattern - when evaluation fails, it returns your specified default value instead of throwing an error.

**📖 See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:

- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### Go-Specific Examples

```go
// The provider returns the default value on errors
value, err := client.BooleanValue(ctx, "my-flag.enabled", false, evalCtx)
if err != nil {
    // Log the error for debugging
    log.Printf("Flag evaluation failed, using default: %v", err)
}
// value will be 'false' if evaluation failed

// For critical flags, you might want to check the error
if err != nil && strings.Contains(err.Error(), "FLAG_NOT_FOUND") {
    log.Warn("Flag 'my-flag' not found in Confidence - check flag name")
}

// During initialization with timeout
ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
defer cancel()

provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret: "your-client-secret",
})
if err != nil {
    log.Fatalf("Provider initialization failed: %v", err)
}
```

## Configuration

### Environment Variables

Configure the provider behavior using environment variables:

- `CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS`: Timeout for remote materialization store read operations (default: `2` seconds)
- `CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS`: Timeout for remote materialization store write operations (default: `5` seconds)

#### Deprecated Environment Variables

The following environment variables are deprecated and will be removed in a future version. Use `ProviderConfig` fields instead:

- `CONFIDENCE_STATE_POLL_INTERVAL_SECONDS`: Use `ProviderConfig.StatePollInterval` instead
- `CONFIDENCE_LOG_POLL_INTERVAL_SECONDS`: Use `ProviderConfig.LogPollInterval` instead

### ProviderConfig

The `ProviderConfig` struct contains all configuration options for the provider:

#### Required Fields

- `ClientSecret` (string): The client secret used for authentication and flag evaluation

#### Optional Fields

- `Logger` (\*slog.Logger): Custom logger for provider operations. If not provided, a default text logger is created. See [Logging](#logging) for details.
- `TransportHooks` (TransportHooks): Custom transport hooks for advanced use cases (e.g., custom gRPC interceptors, HTTP transport wrapping, TLS configuration)
- `StatePollInterval` (time.Duration): Interval for polling flag state updates (default: 10 seconds)
- `LogPollInterval` (time.Duration): Interval for flushing evaluation logs (default: 60 seconds)
- `ResolverPoolSize` (int): Number of WASM resolver instances in the pool (default: `GOMAXPROCS`, i.e., number of CPU cores)
- `MaterializationStore` (MaterializationStore): Storage for sticky variant assignments and materialized segments. Options include:

  - `nil` (default): Falls back to default values for flags requiring materializations
  - `NewRemoteMaterializationStore()`: Uses remote gRPC storage (recommended for getting started)
  - Custom implementation: Your own storage (Redis, DynamoDB, etc.) for optimal performance

  See [Materialization Stores](#materialization-stores) for details.

#### Advanced: Testing with Custom State Provider

For testing purposes only, you can provide a custom `StateProvider` and `FlagLogger` to supply resolver state and control logging behavior:

```go
// WARNING: This is for testing only. Do not use in production.
provider, err := confidence.NewProviderForTest(ctx,
    confidence.ProviderTestConfig{
        StateProvider:     myCustomStateProvider,
        FlagLogger:        myCustomFlagLogger,
        ClientSecret:      "your-client-secret",
        Logger:            myCustomLogger,        // Optional: custom logger
        StatePollInterval: 30 * time.Second,      // Optional: state polling interval
        LogPollInterval:   2 * time.Minute,       // Optional: log flushing interval
    },
)
```

**Important**: This configuration requires you to provide both a `StateProvider` and `FlagLogger`. For production deployments, always use `NewProvider()` with `ProviderConfig`.

## Materialization Stores

Materialization stores provide persistent storage for sticky variant assignments and custom targeting segments. This enables two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.

2. **Custom Targeting via Materialized Segments**: Precomputed sets of identifiers from datasets that should be targeted. Instead of evaluating complex targeting rules at runtime, materializations allow efficient lookup of whether a unit (user, session, etc.) is included in a target segment.

### Default Behavior

⚠️ Warning: If your flags rely on sticky assignments or materialized segments, the default SDK behaviour will prevent those rules from being applied and your evaluations will fall back to default values. For production workloads that need sticky behavior or segment lookups, implement and configure a real `MaterializationStore` (e.g., Redis, Bigtable, DynamoDB) to avoid unexpected fallbacks and ensure consistent variant assignment. ✅

### Remote Materialization Store

For quick setup without managing your own storage infrastructure, enable the built-in remote materialization store. This implementation stores materialization data via gRPC to the Confidence service.

**When to use**:

- You need sticky assignments or materialized segments but don't want to manage storage infrastructure
- Quick prototyping or getting started
- Lower-volume applications where network latency is acceptable

**Trade-offs**:

- Additional network calls during flag resolution (adds latency)
- Lower performance compared to local storage implementations (Redis, DynamoDB, etc.)

```go
import (
    "context"

    "github.com/open-feature/go-sdk/openfeature"
    "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
)

func main() {
    ctx := context.Background()

    // Enable remote materialization store
    provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
        ClientSecret: "your-client-secret",
        UseRemoteMaterializationStore: true,
    })
    if err != nil {
        log.Fatalf("Failed to create provider: %v", err)
    }

    openfeature.SetProviderAndWait(provider)
    // ...
}
```

The remote store is created automatically by the provider with the correct gRPC connection and authentication.

### Custom Implementations

For improved latency and reduced network calls, you can implement your own `MaterializationStore` interface to store materialization data in your infrastructure (Redis, DynamoDB, etc.):

```go
import (
    "context"
    "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
)

// Implement the MaterializationStore interface
type MyRedisStore struct {
    // your implementation
}

func (s *MyRedisStore) Read(ctx context.Context, ops []confidence.ReadOp) ([]confidence.ReadResult, error) {
    // Load materialization data from Redis
}

func (s *MyRedisStore) Write(ctx context.Context, ops []confidence.WriteOp) error {
    // Store materialization data to Redis
}

// Use your custom store
func main() {
    ctx := context.Background()

    myStore := &MyRedisStore{
        // initialize your store
    }

    provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
        ClientSecret:          "your-client-secret",
        MaterializationStore: myStore,
    })
    // ...
}
```

### In-Memory Store for Testing

An in-memory reference implementation is provided for testing and development.
**Warning**: An in-memory store should NOT be used in production because:

- Data is lost on application restart (no persistence)
- Memory grows unbounded
- Not suitable for multi-instance deployments

### When to Use Materialization Stores

Consider implementing a custom materialization store if:

- You need to support sticky variant assignments for experiments
- You use materialized segments for custom targeting
- You want to minimize network latency during flag resolution
- You have high-volume flag evaluations

If you don't use sticky assignments or materialized segments, the default behavior is sufficient.

## Flag Evaluation

The provider supports all OpenFeature value types:

```go
// Boolean flags
enabled, err := client.BooleanValue(ctx, "feature.enabled", false, evalCtx)

// String flags
color, err := client.StringValue(ctx, "feature.button_color", "blue", evalCtx)

// Integer flags
timeout, err := client.IntValue(ctx, "feature.timeout-ms", 5000, evalCtx)

// Float flags
ratio, err := client.FloatValue(ctx, "feature.sampling_ratio", 0.5, evalCtx)

// Object/structured flags
config, err := client.ObjectValue(ctx, "feature", map[string]any{ "some": "value" }, evalCtx)
```

### Typed Default Values

When using `ObjectValue`, the result can **always** be type-asserted to the type of your default value. The resolved flag value must be fully assignable to the default value's type—if not, it's treated as an error and the default value is returned.

> **Note: Field Name Mapping**
>
> Struct field names are automatically converted to `snake_case` for matching (e.g., `ButtonColor` → `button_color`). This differs from Go's standard `encoding/json` package which uses exact field names. Use `json` tags to override the mapping if needed.

```go
type FeatureConfig struct {
    ButtonColor string                 // matches "button_color"
    MaxRetries  int  `json:"retries"`  // matches "retries" (overridden)
    Enabled     bool                   // matches "enabled"
}

defaultConfig := FeatureConfig{
    ButtonColor: "blue",
    MaxRetries:  3,
    Enabled:     false,
}

result, err := client.ObjectValue(ctx, "feature.config", defaultConfig, evalCtx)
config := result.(FeatureConfig)  // Safe: result is always the same type as defaultConfig
```

#### Struct Defaults: All-or-Nothing

With struct defaults, the resolved value must satisfy **all** fields. If even one field is missing or has an incompatible type, the entire resolved value is discarded and you get the default.

```go
// If the flag resolves to: {"button_color": "red", "retries": 5}
// Missing "enabled" field → error, returns defaultConfig entirely
// You never get a mix of resolved + default values
```

This ensures type safety: your code always receives a complete, valid struct.

#### Map Defaults: Merge Semantics

With `map[string]any` defaults, resolved values are **merged** into a copy of the default. However, type safety is still enforced—if a field's type doesn't match, it's an error and the entire default map is returned.

```go
defaultMap := map[string]any{
    "timeout": 30,    // int
    "retries": 3,     // int
}

// If flag resolves to: {"timeout": 60, "extra": true}
// Result: {"timeout": 60, "retries": 3, "extra": true}
// - "timeout" overwritten by resolved value
// - "retries" kept from default (not in resolved)
// - "extra" added from resolved value

// If flag resolves to: {"timeout": "fast"}
// Type mismatch (string vs int) → error, returns defaultMap
```

With `nil` default, you get the resolved value as-is using natural Go types (numbers as `float64`, objects as `map[string]any`, etc.).

#### Assignability Rules

- Struct fields are matched by `json` tag, or snake_case conversion of the field name
- Types must match exactly (a number where a bool is expected → error)
- Numbers convert to integers only if they're whole numbers (3.0 → ok, 3.5 → error)
- Negative numbers cannot convert to unsigned integers
- Null values in the flag use the corresponding default value for that field

## Logging

The provider uses `log/slog` for structured logging. By default, logs at `Info` level and above are written to `stderr`.

You can provide a custom logger to control log level, format, and destination:

```go
import "log/slog"

// JSON logger with debug level
logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
    Level: slog.LevelDebug,
}))

provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret: "your-client-secret",
    Logger:       logger,
})
```

The provider logs at different levels: `Debug` (flag resolution details), `Info` (state updates), `Warn` (non-critical issues), and `Error` (failures).

## Shutdown

**Important**: Always shut down the provider when your application exits to ensure proper cleanup and log flushing.

```go
// Shutdown the provider on application exit
    openfeature.Shutdown()
```

### What Happens During Shutdown?

1. **Flushes pending logs** to Confidence (exposure events, resolve analytics)
2. **Closes gRPC connections** and releases network resources
3. **Stops background tasks** (state polling, log batching)
4. **Releases WASM instance** and memory

The shutdown respects the context timeout you provide.

## License

See the root `LICENSE` file.
