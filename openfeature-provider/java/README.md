# Confidence OpenFeature Local Provider for Java

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using WebAssembly (WASM)
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging (and other resolve analytics)
- **OpenFeature Compatible**: Works with the standard OpenFeature SDK
- **HTTP Proxy Service**: Proxy requests from client SDKs through your backend for enhanced control

## Installation

Add this dependency to your `pom.xml`:
<!-- x-release-please-start-version -->
```xml
<dependency>
    <groupId>com.spotify.confidence</groupId>
    <artifactId>openfeature-provider-local</artifactId>
    <version>0.12.2</version>
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

## HTTP Proxy Service (FlagResolverService)

The `FlagResolverService` enables you to proxy flag resolution requests from client SDKs (like `confidence-sdk-js`) through your backend service. This is useful when you want:

- **Low-latency resolution**: Client SDKs make requests to your backend instead of Confidence servers
- **Backend-controlled credentials**: Client SDKs don't need their own client secrets
- **Context enrichment**: Add server-side context (user ID from auth, request metadata) before resolution
- **JSON only**: Only `application/json` content type is supported. Requests with other content types will receive a 415 response.

### Basic Setup

```java
import com.spotify.confidence.sdk.OpenFeatureLocalResolveProvider;
import com.spotify.confidence.sdk.FlagResolverService;

// Create and initialize the provider
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret");
// If you're not using OpenFeature, don't forget to call provider.initialize()
OpenFeatureAPI.getInstance().setProviderAndWait(provider);

// Create the HTTP service
FlagResolverService flagResolver = new FlagResolverService(provider);
```

### With Context Decoration

Add server-side context to requests before resolution:

```java
FlagResolverService flagResolver = new FlagResolverService(provider,
    ContextDecorator.sync((ctx, req) -> {
        // Set targeting key from upstream auth middleware header
        List<String> userIds = req.headers().get("X-User-Id");
        if (userIds != null && !userIds.isEmpty()) {
            return ctx.merge(new ImmutableContext(userIds.get(0)));
        }
        return ctx;
    }));
```

### Framework Integration

The service uses simple interfaces that can be adapted to any HTTP framework.

#### Servlet Example

```java
public class FlagServlet extends HttpServlet {
    private final FlagResolverService flagResolverService;

    public FlagServlet(OpenFeatureLocalResolveProvider provider) {
        // Set targeting key from upstream auth middleware header
        this.flagResolverService = new FlagResolverService(provider,
            ContextDecorator.sync((context, request) -> {
                List<String> userIds = request.headers().get("X-User-Id");
                if (userIds != null && !userIds.isEmpty()) {
                    return context.merge(new ImmutableContext(userIds.get(0)));
                }
                return context;
            }));
    }

    @Override
    protected void service(HttpServletRequest req, HttpServletResponse resp) throws IOException {
        ConfidenceHttpResponse response;

        if (req.getPathInfo().endsWith("v1/flags:resolve")) {
            response = flagResolverService.handleResolve(toConfidenceRequest(req))
                    .toCompletableFuture().join();
        } else if (req.getPathInfo().endsWith("v1/flags:apply")) {
            response = flagResolverService.handleApply(toConfidenceRequest(req))
                    .toCompletableFuture().join();
        } else {
            resp.setStatus(404);
            return;
        }

        resp.setStatus(response.statusCode());
        response.headers().forEach(resp::setHeader);
        resp.getOutputStream().write(response.body());
    }

    private ConfidenceHttpRequest toConfidenceRequest(HttpServletRequest req) {
        // Cache body bytes since InputStream can only be read once
        final byte[] bodyBytes;
        try {
            bodyBytes = req.getInputStream().readAllBytes();
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }

        return new ConfidenceHttpRequest() {
            @Override
            public String method() {
                return req.getMethod();
            }

            @Override
            public byte[] body() {
                return bodyBytes;
            }

            @Override
            public Map<String, List<String>> headers() {
                Map<String, List<String>> headers = new HashMap<>();
                Collections.list(req.getHeaderNames()).forEach(name ->
                    headers.put(name, Collections.list(req.getHeaders(name))));
                return headers;
            }
        };
    }
}
```

Register the servlet at `/confidence-flags/*` to handle requests to `/confidence-flags/v1/flags:resolve` and `/confidence-flags/v1/flags:apply`.

### Client SDK Configuration

Configure your client SDK (e.g., `confidence-sdk-js`) to use your backend:

```javascript
import { Confidence } from '@spotify-confidence/sdk';

const confidence = Confidence.create({
  clientSecret: 'not-used-but-required',
  resolveBaseUrl: 'https://your-backend.com/confidence-flags',
  applyBaseUrl: 'https://your-backend.com/confidence-flags',
});
```

## Requirements

- Java 17+
- OpenFeature SDK 1.6.1+

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.