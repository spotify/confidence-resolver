# Confidence OpenFeature Provider for Java

> [!NOTE]
> This is the `next` version of the provider, built on compiled flag modules and released as
> release candidates. For the current release, see the provider on `main`.

![Status: Experimental](https://img.shields.io/badge/status-experimental-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates flags locally in a WebAssembly **flag module** compiled from
  your client credential's flags, fetched encrypted from the Confidence CDN
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Picks up a new flag module as your flags change
- **Exposure Logging**: Exposure logging and resolve analytics
- **OpenFeature Compatible**: Works with the standard OpenFeature Java SDK
- **HTTP Proxy Service**: Proxy requests from client SDKs through your backend for enhanced control

Not yet supported in this version: event tracking (`track()`). It will return through the flag
modules.

## Installation

Add this dependency to your `pom.xml`:
<!-- x-release-please-start-version -->
```xml
<dependency>
    <groupId>com.spotify.confidence</groupId>
    <artifactId>openfeature-provider-local</artifactId>
    <version>0.24.0-rc2</version>
</dependency>
```
<!-- x-release-please-end -->

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration

The first startup with a new client credential may take longer while Confidence builds and
publishes its flag module. Until the module is available, initialization behaves as it would if the
CDN were unreachable.

## Encryption

The provider fetches the flag module as an encrypted artifact and decrypts it when loading.
Pass the encryption key for your client credential when creating the provider.

See the [Integration Guide](../INTEGRATION_GUIDE.md#encryption) for background and migration details.

Pass both credentials to the provider constructor:

```java
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret", "your-encryption-key");
```

Each client credential has a unique encryption key, available alongside it in [Confidence Admin](https://app.confidence.spotify.com/admin/clients).

## Quick Start

```java
import com.spotify.confidence.sdk.OpenFeatureLocalResolveProvider;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.exceptions.OpenFeatureError;

// Create and register the provider
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret", "your-encryption-key"); // Get from Confidence dashboard
try {
    OpenFeatureAPI.getInstance().setProviderAndWait(provider); // Important: use setProviderAndWait()
} catch (OpenFeatureError e) {
    // Flag module not installed yet — keep booting on defaults; see "Initialization at startup"
    System.err.println("Confidence provider init failed, serving flag defaults: " + e.getMessage());
}

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

`setProviderAndWait` calls `initialize()`, which installs the credential's first flag module and
rethrows on failure. Evaluations before the first module (and after errors) return your
defaults. See [Initialization at startup](#initialization-at-startup) for the failure modes and
how to handle them at service startup.

## Initialization at startup

`setProviderAndWait` blocks until the credential's first flag module is installed — up to
`initializationTimeout`, 30 seconds by default — and **rethrows** when initialization fails. In
0.23 a failed state load only logged a warning and the provider served defaults; from 0.24 the
failure surfaces at registration, so an unguarded call can stop a service from booting during
an outage of the module CDN (`confidence-resolver-module-cdn.spotifycdn.com`).

Catch the failure and keep booting on defaults — the provider stays registered and evaluations
return your defaults (`PROVIDER_NOT_READY` on the details) until the module installs:

```java
try {
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
} catch (OpenFeatureError e) {
    logger.error("Confidence provider init failed; serving flag defaults", e);
    // OpenFeatureAPI.getInstance().shutdown(); // stay on defaults for the whole process
}
```

- **Timeout** (CDN unreachable, or the credential's module still building): the engine keeps
  retrying in the background and the provider may become ready later — evaluations then switch
  from defaults to resolved values mid-run. If the service must behave consistently for its
  whole lifetime, call `shutdown()` in the catch (as commented above); evaluations then return
  defaults for the rest of the process.
- **Rejected credential**: `FATAL`, no retry — fix the credential and restart. Alert on the
  catch either way.
- Let the exception propagate instead when wrong defaults are more dangerous than downtime
  (compliance, payment logic). Catching is only as safe as your
  [defaults](../INTEGRATION_GUIDE.md#production-best-practices).

Upgrading from `0.23`? See [Migrating to the `next` providers](../MIGRATION.md).

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

The provider uses a **default value fallback** pattern - when evaluation fails, it returns your
specified default value with an error code set on the details, instead of throwing.

Initialization failures behave differently — they throw from `setProviderAndWait`; see
[Initialization at startup](#initialization-at-startup).

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

## Configuration

### Provider Configuration

Pass the client secret and encryption key to the constructor. Use `LocalProviderConfig.builder()` for optional settings:

```java
LocalProviderConfig config = LocalProviderConfig.builder()
    .materializationStore(new MyRedisStore())       // sticky assignments + materialized segments
    .exposureMode(ExposureMode.ALL)                 // log every exposure
    .instances(8)                                   // concurrent evaluation capacity
    .build();

OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider(config, "your-client-secret", "your-encryption-key");
```

| Option | Default | Description |
|--------|---------|-------------|
| `materializationStore(MaterializationStore)` | none (unsupported) | Storage for sticky assignments and materialized segments. See [Materialization Stores](#materialization-stores). |
| `useRemoteMaterializationStore(boolean)` | `false` | Use Confidence's hosted materialization store instead of your own. Mutually exclusive with `materializationStore`. |
| `exposureMode(ExposureMode)` | `DEDUPLICATED` | How exposures are logged. See [Exposure logging](#exposure-logging). |
| `instances(int)` | one per processor | The maximum number of concurrent evaluations. |
| `maxMemoryBytes(long)` | 64 MiB | Caps the flag module's memory. |
| `statePollInterval(Duration)` | 10 seconds | How often to check for a new flag module. |
| `initializationTimeout(Duration)` | 30 seconds | How long `initialize()` waits for the first module. |
| `interpreter(boolean)` | `false` | Run the flag module on the interpreter instead of the compiled runtime. See [WASM interpreter mode](#wasm-interpreter-mode). |
| `httpClientFactory(HttpClientFactory)` | shared `HttpClient` | Hook supplying the `java.net.http.HttpClient` used for every request to Confidence (module CDN, profile registration, flag logs, hosted materialization store). Disable redirects on this client to allow profile registration. Use it for proxies, custom TLS or instrumentation. |
| `logger(Logger)` | SLF4J default | Custom logger. See [Logging](#logging). |

### Environment Variables

- `CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS`: Timeout for materialization read operations when using the remote materialization store (default: `2` seconds)
- `CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS`: Timeout for materialization write operations when using the remote materialization store (default: `5` seconds)

Everything else is configured through `LocalProviderConfig.builder()`.

## Exposure logging

By default the provider logs each unit's exposure to a variant once per deduplication window
(`ExposureMode.DEDUPLICATED`), so repeatedly evaluating the same flag for the same targeting key
produces one exposure event instead of many. Distinct units are never collapsed, so per-unit
exposure counts are unaffected.

`exposureMode` changes this for every evaluation through the provider:

| Mode | Exposure events |
| --- | --- |
| `DEDUPLICATED` (default) | Once per unit and variant per window |
| `ALL` | Every evaluation |
| `NONE` | None (resolve analytics are still sent) |

```java
LocalProviderConfig config = LocalProviderConfig.builder()
    .exposureMode(ExposureMode.ALL)
    .build();
```

## WASM interpreter mode

By default the flag module is compiled to JVM bytecode on first load (Endive's compiler) for
near-native evaluation speed. `interpreter(true)` runs the module on the interpreter instead.

### When to enable interpreter mode

- Environments where runtime code generation is restricted (some security managers,
  hardened JVMs)
- Troubleshooting module loading: interpreter-mode failures come with clearer stack traces

### Performance impact

The interpreter is roughly an order of magnitude slower per evaluation. For typical flag
evaluation rates this is still sub-millisecond, but high-QPS services should stay on the
default.

### Example

```java
LocalProviderConfig config = LocalProviderConfig.builder()
    .interpreter(true)
    .build();
```

## Materialization Stores

The provider supports **materializations** for two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.
**📖 See the [Integration Guide: Sticky Assignments](../INTEGRATION_GUIDE.md#sticky-assignments)** for how sticky assignments work and their benefits.

2. **Custom Targeting via Materialized Segments**: Efficiently target precomputed sets of identifiers from datasets. Instead of evaluating complex targeting rules at runtime, materializations allow for fast lookups of whether a unit (user, session, etc.) is included in a target segment.

### Materialization Storage Options

#### 1. No Materialization Support (Default)

By default, materializations are not supported. If a flag requires materialization data (sticky
assignments or custom targeting), the evaluation returns your default value with a `GENERAL`
error code; materialized segments still resolve through the membership data compiled into the
module.

```java
// Default behavior - no materialization support
OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider("your-client-secret", "your-encryption-key");
```

#### 2. Remote Materialization Store

Enable the hosted materialization store to have Confidence manage materialization data
server-side. Reads and writes speak protobuf over HTTPS to the resolver gateway, authenticated
with the client secret:

```java
// Enable the hosted materialization store
LocalProviderConfig config = LocalProviderConfig.builder()
    .useRemoteMaterializationStore(true)
    .build();

OpenFeatureLocalResolveProvider provider =
    new OpenFeatureLocalResolveProvider(config, "your-client-secret", "your-encryption-key");
```

- Requires network calls for materialization reads/writes during flag evaluation
- Automatically handles TTL and cleanup
- Requires no additional infrastructure

#### 3. Custom Materialization Storage

For advanced use cases requiring minimal latency, implement a custom `MaterializationStore` to manage materialization data in your own infrastructure (Redis, database, etc.):

```java
// Custom storage for materialization data
MaterializationStore store = new RedisMaterializationStore(jedisPool);
OpenFeatureLocalResolveProvider provider = new OpenFeatureLocalResolveProvider(
    LocalProviderConfig.builder().build(),
    "your-client-secret",
    "your-encryption-key",
    store
);
```

For detailed information on how to implement custom storage backends, see [STICKY_RESOLVE.md](STICKY_RESOLVE.md).
See the `InMemoryMaterializationStoreExample` class in the test sources for a reference implementation, or review the `MaterializationStore` javadoc for detailed API documentation.

## Flag Evaluation

Flag keys carry a path: `my-flag.enabled` resolves flag `my-flag` and evaluates its `enabled`
field. When a flag has no assignment for the context, the module merges your default into the
resolved value.

### Typed Default Values

Every typed evaluation encodes your default value with its schema, so the module can merge it
into nulls and verify types:

- `Boolean`, `String`, `Integer`, `Double` defaults — scalar schemas
- `Structure` default — per-field schemas; **all-or-nothing**: every field must have a schema
  representation (`Boolean`/`String`/`Integer`/`Double`/nested `Structure`), and a `null` or
  `List` field makes the whole default a type mismatch
- `List` and `Instant` defaults are not supported (they have no schema representation)

The decoded value is type-checked against the default's shape everywhere — including fields
under structures, where the wire schema cannot express the expected type. A mismatch anywhere
returns the default with `TYPE_MISMATCH`.

## Direct Resolve API

Resolve flags by name and receive the full response, or defer applying exposures to a later
`applyFlags` call:

### Resolve with immediate apply

```java
ResolveFlagsResponse response = provider
    .resolve(evaluationContext, List.of("my-flag"), true)
    .toCompletableFuture()
    .join();
```

### Deferred apply

```java
ResolveFlagsResponse response = provider
    .resolve(evaluationContext, List.of("my-flag"), false) // apply = false
    .toCompletableFuture()
    .join();

provider.applyFlags(ApplyFlagsRequest.newBuilder()
    .setResolveToken(response.getResolveToken())
    .setSendTime(Timestamps.now())
    .addFlags(AppliedFlag.newBuilder()
        .setFlag("flags/my-flag")
        .setApplyTime(Timestamps.now()))
    .build());
```

> [!WARNING]
> The resolve token is **neither encrypted nor authenticated**. Treat it as untrusted input
> when handling apply requests proxied from clients. If its contents are sensitive, encrypt
> the token in your proxy before returning it to the client SDK and decrypt it again on the
> apply endpoint before forwarding to `FlagResolverService`.

`applyFlags` is best-effort: rejected applies (e.g. a rotated client secret) are swallowed and
logged, never thrown.

## Metrics

The flag module exposes cumulative telemetry as Prometheus text:

```java
String prometheus = provider.getPrometheusMetrics(new SnapshotConfig());
String openMetrics = provider.getPrometheusMetrics(new SnapshotConfig(true)); // # HELP/# TYPE lines
```

## Logging

Pass an SLF4J `Logger` via the config for structured logging of module swaps, shutdown
warnings and apply failures:

```java
LocalProviderConfig config = LocalProviderConfig.builder()
    .logger(LoggerFactory.getLogger("confidence"))
    .build();
```

## Shutdown

Call `shutdown()` when your application exits to flush exposure logs and clean up resources.
The provider flushes and releases within 10 seconds; shutdown is idempotent:

```java
// Shutdown the provider to flush logs and clean up resources
OpenFeatureAPI.getInstance().shutdown();
```

> **Note**: OpenFeature Java SDK 1.20.2+ properly awaits provider shutdown ([#1744](https://github.com/open-feature/java-sdk/pull/1744)). If you are using an older SDK version (< 1.20.2), call `OpenFeatureAPI.getInstance().getProvider().shutdown()` directly instead to avoid losing exposure logs.

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
    new OpenFeatureLocalResolveProvider("your-client-secret", "your-encryption-key");
// If you're not using OpenFeature, don't forget to call provider.initialize()
// Guard this call as in "Initialization at startup" — it throws when initialization fails
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

- Java 17 or later
- A Confidence backend integration (client secret + encryption key)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for code formatting (`mvn fmt:format`) and the root
README for the provider Makefile contract.

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
