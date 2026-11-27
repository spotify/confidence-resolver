# Confidence OpenFeature Provider for Go

> [!NOTE]
> This is the `next` version of the provider, built on compiled flag modules and released as
> release candidates. For the current release, see the provider on `main`.

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates flags locally in a WebAssembly module compiled from your
  client's flags, fetched from the Confidence CDN
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Picks up new flag modules as flags change
- **Exposure Logging**: Exposure logging and resolve analytics
- **OpenFeature Compatible**: Works with the standard OpenFeature Go SDK

Not yet supported in this version: event tracking (`Track()`).

## Installation

```bash
go get github.com/spotify/confidence-resolver/openfeature-provider/go
go mod tidy
```

## Requirements

- Go 1.25+
- OpenFeature Go SDK 1.16.0+

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:

- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Encryption

The provider fetches its flag module encrypted and decrypts it before loading it. Pass the encryption key for your client credential when creating the provider.

See the [Integration Guide](../INTEGRATION_GUIDE.md#encryption) for background and migration details.

Pass the encryption key in `ProviderConfig`:

```go
provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret:  "your-client-secret",
    EncryptionKey: "your-encryption-key", // Get from Confidence Admin view
})
```

Each client credential has a unique encryption key, available alongside it in [Confidence Admin](https://app.confidence.spotify.com/admin/clients).

## Quick Start

The first startup with a new client credential may take longer while its flag module is built.

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
        EncryptionKey: "your-encryption-key",
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

Initialization uses one total 30-second budget by default, including state requests and one-second retry delays. `SetProviderAndWait` returns `nil` as soon as valid state is installed. If the budget expires, it returns a recoverable timeout error; evaluations use caller-supplied defaults while the provider keeps retrying in the background. Successful background recovery emits `PROVIDER_READY` and resumes the configured state polling interval.

Network failures, timeouts, HTTP 408/429/5xx, and HTTP 404 are retried; 404 remains retryable to allow state provisioning. Other HTTP 4xx responses (including 401/403), decryption/decoding failures, missing account IDs, and rejected resolver state stop startup retries and report `FATAL` with the cause. Correct the configuration or payload and register a new provider to recover from a fatal startup error.

Once valid state has been installed, **any** refresh failure preserves the last working state. The provider remains `READY`, evaluations continue using cached flags, and polling continues. Rejected state, including subsequent 304 responses for those rejected bytes, never replaces the last working state.

### Initialization at startup

`SetProviderAndWait` returns the initialization error, so the choice of
failing fast or serving defaults is yours at the call site:

```go
provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret:          "[REDACTED assignment-style secret]",
    EncryptionKey:         "your-encryption-key",
    InitializationTimeout: 45 * time.Second, // default: 30 seconds
})
if err != nil {
    log.Fatalf("Provider creation failed: %v", err)
}
if err := openfeature.SetProviderAndWait(provider); err != nil {
    log.Printf("Confidence provider init failed; serving flag defaults: %v", err)
    // openfeature.Shutdown() // stay on defaults for the whole process
}
```

For most services this is the right default: a CDN outage degrades the service instead of
preventing it from starting. The SDK keeps retrying in the background and the provider may
become ready later — evaluations then switch from defaults to resolved values mid-run; call
`openfeature.Shutdown()` in the error path (as commented above) if the service must stay on
defaults for its whole lifetime. A rejected credential is `FATAL` and never retries — fix it
and restart. Let the error propagate instead when wrong defaults are more dangerous than
downtime; catching is only as safe as your defaults.

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
```

## Configuration

### ProviderConfig

#### Required Fields

- `ClientSecret` (string): The client secret used for authentication and flag evaluation
- `EncryptionKey` (string): The credential's encryption key, 64 hexadecimal characters. Found in the [Confidence Admin view](https://app.confidence.spotify.com/admin/clients).

#### Optional Fields

- `MaterializationStore` (MaterializationStore): Storage for sticky assignments and materialized segments. See [Materialization Stores](#materialization-stores).
- `UseRemoteMaterializationStore` (bool): Use Confidence's hosted materialization store instead of your own. Requires `MaterializationStore` to be nil.
- `ExposureMode` (ExposureMode): How exposure events are collected (default `ExposureDeduplicated`). See [Exposure logging](#exposure-logging).
- `Instances` (int): The maximum number of concurrent evaluations (default: `GOMAXPROCS`).
- `MaxMemoryBytes` (uint64): Caps the resolver's memory (default: 64 MiB).
- `StatePollInterval` (time.Duration): How often to check for flag updates (default: 10 seconds).
- `InitializationTimeout` (time.Duration): How long initialization waits for the first flag module (default: 30 seconds).
- `UseWasmInterpreter` (bool): Run the flag module in wazero's interpreter instead of its compiler (default: `false`). See [WASM interpreter mode](#wasm-interpreter-mode).
- `HTTPClient` (\*http.Client): Used for all requests to Confidence (default: `http.DefaultClient`). Use it for proxies, custom TLS or instrumentation.
- `Logger` (\*slog.Logger): Custom logger. See [Logging](#logging).

## Exposure logging

By default the provider logs each unit's exposure to a variant once per deduplication window
(`ExposureDeduplicated`), so repeatedly evaluating the same flag for the same targeting key
produces one exposure event instead of many. Distinct units are never collapsed, so per-unit
exposure counts are unaffected.

`ExposureMode` changes this for every evaluation through the provider:

| Mode | Exposure events |
| --- | --- |
| `ExposureDeduplicated` (default) | Once per unit and variant per window |
| `ExposureAll` | Every evaluation |
| `ExposureNone` | None (error events are still sent) |

```go
provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret:  "your-client-secret",
    EncryptionKey: "your-encryption-key",
    ExposureMode:  confidence.ExposureAll,
})
```

Only use `ExposureNone` for exceptional cases: without exposures, experiment results and exposure
counts for those evaluations are incomplete. Resolve analytics are still sent.

To skip exposure for a single evaluation, set `_confidence_skip_apply` to `true` in the evaluation
context:

```go
evalCtx := openfeature.NewEvaluationContext("user-123", map[string]any{
    "_confidence_skip_apply": true,
})

value, err := client.BooleanValue(ctx, "my-flag.enabled", false, evalCtx)
```

The provider removes the key before resolving, so it never reaches targeting or the logs. Other
values than `true` are ignored.

| Mechanism | Scope | Exposure events | Resolve analytics |
| --- | --- | --- | --- |
| `ExposureMode` provider config | Every evaluation through the provider | As the mode says | Still sent |
| `_confidence_skip_apply` context key | One evaluation | None for that evaluation | Still sent |

The context key is intended for exceptional cases. If you're considering it, reach out to the
Confidence team to discuss the best approach for your setup.

## WASM interpreter mode

By default the Go provider runs the flag module through [wazero](https://wazero.io/)'s **JIT compiler** (Wazevo), which compiles WASM to native code for low-latency flag evaluation. On **linux/arm64**, CPU profiling (`SIGPROF`) or goroutine preemption (`SIGURG`) can arrive while a thread is executing that JIT code. Go cannot attribute the signal to a goroutine in that state and may exit the process silently (exit code 139, no stack trace).

Setting `UseWasmInterpreter: true` switches wazero to its **interpreter** engine. WASM runs as ordinary Go code on a normal goroutine stack, so profiling and preemption signals are handled safely. This is an **opt-in safety valve** — keep the default unless you need it.

### When to enable interpreter mode

Enable `UseWasmInterpreter: true` if your service:

- Runs on **linux/arm64** with continuous or frequent CPU profiling (for example Datadog, pprof, or similar agents)
- Has observed **silent process crashes** (exit code 139, no Go traceback) correlated with flag evaluation volume
- Prioritizes **stability over raw evaluation throughput** on affected pods

Keep the default (`UseWasmInterpreter: false`) for:

- Most production workloads without the signal issue above
- Services where flag evaluation latency or throughput is critical
- Environments where profiling does not sample during WASM execution

Go interpreter mode is the slowest local execution path, but it is the practical in-process fix for the arm64 signal issue.

### Performance impact

Interpreter mode is substantially slower than the compiler, on the order of 50× at the WASM
layer. End-to-end request latency depends on how often and how centrally you evaluate flags: a
single evaluation per multi-millisecond workflow step may be acceptable; tight loops or very high
QPS per pod may not.

### Example

```go
provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret:       "your-client-secret",
    EncryptionKey:      "your-encryption-key",
    UseWasmInterpreter: true, // signal-safe on linux/arm64; slower than JIT
})
```

## Materialization Stores

Materialization stores provide persistent storage for sticky variant assignments and custom targeting segments. This enables two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.

2. **Custom Targeting via Materialized Segments**: Precomputed sets of identifiers from datasets that should be targeted. Instead of evaluating complex targeting rules at runtime, materializations allow efficient lookup of whether a unit (user, session, etc.) is included in a target segment.

### Default Behavior

⚠️ Warning: If your flags rely on sticky assignments or materialized segments, the default SDK behaviour will prevent those rules from being applied and your evaluations will fall back to default values. For production workloads that need sticky behavior or segment lookups, implement and configure a real `MaterializationStore` (e.g., Redis, Bigtable, DynamoDB) to avoid unexpected fallbacks and ensure consistent variant assignment. ✅

### Remote Materialization Store

For quick setup without managing your own storage infrastructure, enable the built-in remote materialization store. This implementation reads and writes materialization data through the Confidence service over HTTP.

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
        EncryptionKey: "your-encryption-key",
        UseRemoteMaterializationStore: true,
    })
    if err != nil {
        log.Fatalf("Failed to create provider: %v", err)
    }

    openfeature.SetProviderAndWait(provider)
    // ...
}
```

The provider creates the remote store itself, authenticated with your client secret.

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
        EncryptionKey: "your-encryption-key",
        MaterializationStore: myStore,
    })
    // ...
}
```

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

## Direct Resolve API

In addition to the standard OpenFeature evaluation methods, the provider exposes a lower-level `Resolve` / `ApplyFlags` API for use cases that need more control — for example, resolving multiple flags in a single call or deferring exposure logging.

### Resolve with immediate apply

```go
provider, _ := confidence.NewProvider(ctx, confidence.ProviderConfig{
    ClientSecret: "your-client-secret",
    EncryptionKey: "your-encryption-key",
})
openfeature.SetProviderAndWait(provider)

evalCtx := openfeature.FlattenedContext{
    "targeting_key": "user-123",
    "country":       "US",
}

// Resolve one or more flags at once with apply=true to record exposures immediately.
// Pass an empty slice to resolve all flags available to the client.
resp, err := provider.Resolve(ctx, evalCtx, []string{"flag-a", "flag-b"}, true)
if err != nil {
    log.Fatal(err)
}
for _, f := range resp.ResolvedFlags {
    log.Printf("%s → %s", f.Flag, f.Variant)
}
```

### Deferred apply

When `apply` is `false`, the response contains a `ResolveToken`. Pass it to `ApplyFlags` later to record exposure events — useful when you resolve flags on the server but only want to log exposure once the client actually renders the experience.

> **⚠️ The `ResolveToken` is neither encrypted nor authenticated.** It's a plain serialization of
> the exposure events it stands for: the resolve ID, and for each flag the targeting key and the
> variant, rule and segment it resolved to. Other evaluation context attributes aren't in it.
> Anyone who can read the token sees that, and anyone who can change it before it reaches
> `ApplyFlags` can make the provider log exposures that never happened. The encryption key doesn't
> protect it: that key only decrypts the flag module.
>
> If the token leaves your backend, wrap it in authenticated encryption that only your backend can
> undo (such as AES-GCM with a key of your own) and unwrap it before `ApplyFlags`, or keep the token
> on the server and hand out a reference to it. See the
> [Integration Guide: Deferred Apply and Resolve Token Security](../INTEGRATION_GUIDE.md#deferred-apply-and-resolve-token-security).

```go
// 1. Resolve without applying
resp, err := provider.Resolve(ctx, evalCtx, []string{"checkout-flow"}, false)
if err != nil {
    log.Fatal(err)
}

// 2. … later, after the user has been exposed …
err = provider.ApplyFlags(&resolver.ApplyFlagsRequest{
    Flags: []*resolver.AppliedFlag{
        {Flag: "flags/checkout-flow", ApplyTime: timestamppb.Now()},
    },
    ResolveToken: resp.ResolveToken, // the client secret is filled in by the provider
    SendTime:     timestamppb.Now(),
})
if err != nil {
    log.Printf("apply failed: %v", err)
}
```

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
    EncryptionKey: "your-encryption-key",
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

1. **Stops background tasks** (flag polling, timed log flushes)
2. **Flushes pending logs** to Confidence (exposure events, resolve analytics)
3. **Releases the flag module** and its memory

## License

See the root `LICENSE` file.

### Migrating to mandatory encryption

`EncryptionKey` is now required when constructing the provider. Supply exactly 64
hexadecimal characters. Missing, empty, or malformed keys fail before network
activity. The provider only fetches encrypted state and never falls back to plaintext.

Open [Confidence Admin → Clients](https://app.confidence.spotify.com/admin/clients),
select your client, and locate the credential used by the provider. Each credential
has its own unique encryption key, available alongside it. Use the key paired with
your configured client secret. Configure and verify encryption on your existing
SDK before upgrading.

