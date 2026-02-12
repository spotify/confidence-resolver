# Confidence OpenFeature Local Provider for Java

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using WebAssembly (WASM)
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging (and other resolve analytics)
- **OpenFeature Compatible**: Works with the standard OpenFeature SDK

## Installation

Add this dependency to your `pom.xml`:
<!-- x-release-please-start-version -->
```xml
<dependency>
    <groupId>com.spotify.confidence</groupId>
    <artifactId>openfeature-provider-local</artifactId>
    <version>0.11.3</version>
</dependency>
```
<!-- x-release-please-end -->

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Quick Start

```java
import com.spotify.confidence.OpenFeatureLocalResolveProvider;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.MutableContext;

// Create and register the provider
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret"); // Get from Confidence dashboard
OpenFeatureAPI.getInstance().setProviderAndWait(provider); // Important: use setProviderAndWait()

// Get a client
Client client = OpenFeatureAPI.getInstance().getClient();

// Create evaluation context with user attributes for targeting
MutableContext ctx = new MutableContext("user-123");
ctx.add("country", "US");
ctx.add("plan", "premium");

// Evaluate a flag
Boolean enabled = client.getBooleanValue("test-flag.enabled", false, ctx);
System.out.println("Flag value: " + enabled);

// Don't forget to shutdown when your application exits (see Shutdown section)
```

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### Java-Specific Examples

```java
// Simple attributes
MutableContext ctx = new MutableContext("user-123");
ctx.add("country", "US");
ctx.add("plan", "premium");
ctx.add("age", 25);
```

## Error Handling

The provider uses a **default value fallback** pattern - when evaluation fails, it returns your specified default value instead of throwing an error.

**📖 See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:
- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### Java-Specific Examples

```java
// The provider returns the default value on errors
Boolean enabled = client.getBooleanValue("my-flag.enabled", false, ctx);
// enabled will be 'false' if evaluation failed

// For detailed error information, use getBooleanDetails()
FlagEvaluationDetails<Boolean> details = client.getBooleanDetails("my-flag.enabled", false, ctx);
if (details.getErrorCode() != null) {
    System.err.println("Flag evaluation error: " + details.getErrorMessage());
    System.err.println("Reason: " + details.getReason());
}
```

## Shutdown

**Important**: To ensure proper cleanup and flushing of exposure logs, you must call `shutdown()` on the provider instance rather than using `OpenFeatureAPI.getInstance().shutdown()`.

```java
// Shutdown the provider to flush logs and clean up resources
OpenFeatureAPI.getInstance().getProvider().shutdown();
```

> **Why?** Due to an [upstream issue in the OpenFeature Java SDK](https://github.com/open-feature/java-sdk/issues/1745), calling `OpenFeatureAPI.getInstance().shutdown()` submits provider shutdown tasks to an executor but doesn't wait for them to complete. This can result in loss of exposure logs and other telemetry data. Calling `shutdown()` directly on the provider ensures proper cleanup.

## Configuration

### Environment Variables

Configure the provider behavior using environment variables:

- `CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS`: How often to poll Confidence to get updates (default: `30` seconds)
- `CONFIDENCE_NUMBER_OF_WASM_INSTANCES`: How many WASM instances to create (this defaults to `Runtime.getRuntime().availableProcessors()` and will affect the performance of the provider)
- `CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS`: Timeout for materialization read operations when using remote materialization store (default: `2` seconds)
- `CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS`: Timeout for materialization write operations when using remote materialization store (default: `5` seconds)

##### Deprecated in favour of a custom ChannelFactory:
- `CONFIDENCE_DOMAIN`: Override the default Confidence service endpoint (default: `edge-grpc.spotify.com`)
- `CONFIDENCE_GRPC_PLAINTEXT`: Use plaintext gRPC connections instead of TLS (default: `false`)

### Custom Channel Factory (Advanced)

For testing or advanced production scenarios, you can provide a custom `ChannelFactory` to control how gRPC channels are created:

```java
import com.spotify.confidence.LocalProviderConfig;
import com.spotify.confidence.ChannelFactory;

// Example: Custom channel factory for testing with in-process server
ChannelFactory mockFactory = (target, interceptors) ->
    InProcessChannelBuilder.forName("test-server")
        .usePlaintext()
        .intercept(interceptors.toArray(new ClientInterceptor[0]))
        .build();

LocalProviderConfig config = new LocalProviderConfig(mockFactory);
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider(config, "client-secret");
```

This is particularly useful for:
- **Unit testing**: Inject in-process channels with mock gRPC servers
- **Integration testing**: Point to local test servers
- **Production customization**: Custom TLS settings, proxies, or connection pooling
- **Debugging**: Add custom logging or tracing interceptors

## Materializations

The provider supports **materializations** for two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.
**📖 See the [Integration Guide: Sticky Assignments](../INTEGRATION_GUIDE.md#sticky-assignments)** for how sticky assignments work and their benefits.

1. **Custom Targeting via Materialized Segments**: Efficiently target precomputed sets of identifiers from datasets. Instead of evaluating complex targeting rules at runtime, materializations allow for fast lookups of whether a unit (user, session, etc.) is included in a target segment.

### Materialization Storage Options

The provider offers three options for managing materialization data:

#### 1. No Materialization Support (Default)

By default, materializations are not supported. If a flag requires materialization data (sticky assignments or custom targeting), the evaluation will fail and return the default value.

```java
// Default behavior - no materialization support
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret");
```

#### 2. Remote Materialization Store

Enable remote materialization storage to have Confidence manage materialization data server-side. When sticky assignment data or materialized segment targeting data is needed, the provider makes a gRPC call to Confidence:

```java
// Enable remote materialization storage
LocalProviderConfig config = LocalProviderConfig.builder()
    .useRemoteMaterializationStore(true)
    .build();

OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider(config, "your-client-secret");
```

**⚠️ Important Performance Impact**: This option fundamentally changes how the provider operates:
- **Without materialization (default)**: Flag evaluation requires **zero network calls** - all evaluations happen locally with minimal latency
- **With remote materialization**: Flag evaluation **requires network calls** to Confidence for materialization reads/writes, adding latency to each evaluation

This option:
- Requires network calls for materialization reads/writes during flag evaluation
- Automatically handles TTL and cleanup
- Requires no additional infrastructure
- Suitable for production use cases where sticky assignments or materialized segment targeting are required

#### 3. Custom Materialization Storage

For advanced use cases requiring minimal latency, implement a custom `MaterializationStore` to manage materialization data in your own infrastructure (Redis, database, etc.):

```java
// Custom storage for materialization data
MaterializationStore store = new RedisMaterializationStore(jedisPool);
OpenFeatureLocalResolveProvider provider = new OpenFeatureLocalResolveProvider(
    new LocalProviderConfig(),
    "your-client-secret",
    store
);
```

For detailed information on how to implement custom storage backends, see [STICKY_RESOLVE.md](STICKY_RESOLVE.md).
See the `InMemoryMaterializationStoreExample` class in the test sources for a reference implementation, or review the `MaterializationStore` javadoc for detailed API documentation.

## Multi-Flag Resolve (Advanced and Experimental)

> ⚠️ **Experimental**: This feature is experimental and may change.

The provider exposes a `resolve()` method that resolves multiple flags in a single call with deferred application (exposure logging). This is useful when building a backend service that proxies flag resolution for client SDKs.

### Use Case: Backend Proxy for Legacy Clients

This feature enables you to build resilient architectures where your backend service handles flag resolution on behalf of client SDKs that don't have local resolve capability. Benefits include:

- **Increased Resilience**: Client SDKs don't need direct connectivity to Confidence
- **Reduced Client Latency**: Your backend can pre-resolve flags before the client needs them
- **Centralized Control**: All flag resolution flows through your infrastructure
- **Legacy SDK Support**: Works with any client SDK that can make HTTP requests

### Prerequisites

To use this feature, you need:

1. **A compatible client SDK version**: The client SDK must support configuring custom base URLs for **both** resolve and apply requests. Not all SDK versions support this — check your SDK's documentation for `resolveBaseUrl` / `applyBaseUrl` configuration options (or equivalent).

2. **Backend endpoints exposed in your environment**: You must implement and expose HTTP endpoints in your backend service that the client SDK can call. These endpoints will:
   - Receive resolve requests from the client SDK and forward them to the Java provider
   - Receive apply requests from the client SDK and forward them to the Java provider
   - Handle any authentication/authorization between your client and backend

### How It Works

```
┌─────────────┐     resolve      ┌─────────────────┐    (local WASM)    ┌────────────┐
│  Client SDK │ ───────────────► │  Your Backend   │ ◄───────────────── │ Confidence │
│             │                  │  (Java Provider)│     sync flags     │  Service   │
│             │ ◄─────────────── │                 │                    │            │
│             │  values + token  │                 │                    │            │
│             │                  │                 │                    │            │
│             │     apply        │                 │    log exposure    │            │
│             │ ───────────────► │                 │ ─────────────────► │            │
└─────────────┘                  └─────────────────┘                    └────────────┘
```

1. Client SDK sends a resolve request to _your_ backend endpoint.
2. Your backend calls `provider.resolve(context, flagNames)` with `apply=false`
3. Your backend returns the resolved values and `resolveToken` to the client
4. When the Client SDK renders/uses a flag value, it sends an apply request to _your_ endpoint.
5. Your backend calls `provider.applyFlag(resolveToken, flagName)` to log exposure

### Important: Client Secret Behavior

**The Java provider's client secret is used for all flag resolution**, not the client SDK's secret. This means:

- The client SDK's credentials are only used for authenticating with *your* backend service
- The provider resolves flags based on the client/secret it was initialized with
- All flags enabled for the provider's client will be available, regardless of what client the SDK claims to be

This is by design — it allows you to use a single backend client for resolution while your client SDKs authenticate with your service using their own credentials.

### Example Implementation

```java
// Initialize provider with YOUR backend client secret
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-backend-client-secret");
OpenFeatureAPI.getInstance().setProviderAndWait(provider);

// In your resolve endpoint handler:
public ResolveResponse handleResolve(ResolveRequest request) {
    // Build evaluation context from client request
    MutableContext ctx = new MutableContext(request.getTargetingKey());
    request.getAttributes().forEach(ctx::add);

    // Resolve flags WITHOUT applying (apply=false is the default)
    ResolveFlagsResponse response = provider.resolve(ctx, request.getFlagNames());

    // Return resolved values and token to client
    return new ResolveResponse(
        response.getResolvedFlagsList(),
        response.getResolveToken().toByteArray(),
        response.getResolveId()
    );
}

// In your apply endpoint handler:
public void handleApply(ApplyRequest request) {
    // Log exposure for flags the client actually used
    for (String flagName : request.getAppliedFlags()) {
        provider.applyFlag(
            ByteString.copyFrom(request.getResolveToken()),
            flagName
        );
    }
}
```

### API Note

The `resolve()` method returns `ResolveFlagsResponse` and `applyFlag()` accepts `ByteString` — both are protobuf types from the shaded `com.spotify.confidence.sdk.shaded.com.google.protobuf` package. If you're serializing these for client communication, use `.toByteArray()` and `ByteString.copyFrom()` to convert to/from `byte[]`.

## Requirements

- Java 17+
- OpenFeature SDK 1.18.2+

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.