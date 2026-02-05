# Confidence Android OpenFeature Provider

Local-resolution OpenFeature provider for Android that evaluates Confidence feature flags using a WebAssembly (WASM) resolver.

## Features

- **Local Resolution**: Evaluates flags locally using an embedded WASM resolver for fast, low-latency flag evaluation
- **Automatic State Sync**: Periodically syncs flag configurations from the Confidence CDN
- **Static Context Support**: Designed for Android's static context pattern where evaluation context is set once and reused
- **Sticky Assignments**: Supports maintaining consistent variant assignments via materialization stores
- **Background Processing**: Uses Kotlin coroutines for non-blocking state polling and log flushing

## Requirements

- Android API 26+ (Android 8.0 Oreo)
- Kotlin 1.9+

## Installation

Add the dependency to your `build.gradle.kts`:

```kotlin
dependencies {
    implementation("com.spotify.confidence:confidence-android-provider:0.1.0")
}
```

## Usage

### Basic Setup

```kotlin
import com.spotify.confidence.android.ConfidenceLocalProvider
import dev.openfeature.sdk.OpenFeatureAPI
import dev.openfeature.sdk.ImmutableContext
import dev.openfeature.sdk.Value

// Create the provider
val provider = ConfidenceLocalProvider("your-client-secret")

// Set global evaluation context (Android static context pattern)
OpenFeatureAPI.setEvaluationContext(
    ImmutableContext(
        targetingKey = "user-123",
        attributes = mapOf(
            "country" to Value.String("US"),
            "premium" to Value.Boolean(true)
        )
    )
)

// Initialize the provider
OpenFeatureAPI.setProviderAndWait(provider)

// Get a client and resolve flags
val client = OpenFeatureAPI.getClient()
val showFeature = client.getBooleanValue("show-new-feature", false)
val buttonColor = client.getStringValue("button-color", "blue")
```

### Advanced Configuration

```kotlin
import com.spotify.confidence.android.LocalProviderConfig
import java.time.Duration

val config = LocalProviderConfig.Builder()
    .statePollInterval(Duration.ofSeconds(60))  // How often to check for state updates
    .logFlushInterval(Duration.ofSeconds(30))    // How often to flush flag logs
    .useRemoteMaterializationStore(true)         // Enable remote sticky assignments
    .build()

val provider = ConfidenceLocalProvider(config, "your-client-secret")
```

### Nested Flag Values

Access nested values using dot notation:

```kotlin
// If your flag has a structure like: { "button": { "color": "red", "size": 12 } }
val buttonColor = client.getStringValue("my-flag.button.color", "blue")
val buttonSize = client.getIntegerValue("my-flag.button.size", 10)
```

### Shutdown

Clean up resources when the provider is no longer needed:

```kotlin
provider.shutdown()
```

## Static Context Pattern

Android's OpenFeature SDK uses a static context pattern, meaning:

1. The evaluation context is set once during app initialization
2. This context is reused for all subsequent flag evaluations
3. Context updates are applied to all future evaluations

This differs from server-side SDKs where context is passed per-request. The Confidence provider fully supports this pattern:

```kotlin
// Set context once at startup
OpenFeatureAPI.setEvaluationContext(
    ImmutableContext(
        targetingKey = userId,
        attributes = userAttributes
    )
)

// All evaluations use this context
val feature1 = client.getBooleanValue("feature-1", false)
val feature2 = client.getStringValue("feature-2", "default")

// Update context when user info changes
OpenFeatureAPI.setEvaluationContext(
    ImmutableContext(
        targetingKey = newUserId,
        attributes = newUserAttributes
    )
)
```

## Architecture

The provider consists of these components:

- **ConfidenceLocalProvider**: Main OpenFeature provider implementation
- **WasmResolver**: Interfaces with the Confidence WASM resolver binary via Chicory
- **SwapWasmResolverApi**: Manages WASM instance lifecycle with hot-swapping support
- **StateFetcher**: Fetches and caches resolver state from CDN with ETag support
- **GrpcWasmFlagLogger**: Sends flag resolution logs via gRPC
- **MaterializationStore**: Interface for sticky assignment storage
- **ConfidenceValue**: Type-safe value system compatible with Confidence SDK
- **ValueConversions**: Utilities for converting between OpenFeature and Confidence types
- **TypeMapper**: Converts between Protobuf and OpenFeature types

## Compatibility with Existing Confidence SDK

This provider is designed to be compatible with the [Confidence SDK Android](https://github.com/spotify/confidence-sdk-android).
It uses the same:
- Value type system (ConfidenceValue)
- Context handling patterns
- Initialization strategies

The main difference is that this provider uses local WASM-based resolution instead of remote HTTP/gRPC calls.

## Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `statePollInterval` | How often to poll for state updates | 30 seconds |
| `logFlushInterval` | How often to flush flag logs | 10 seconds |
| `initialisationStrategy` | How to initialize (FetchAndActivate or ActivateAndFetchAsync) | FetchAndActivate |
| `useRemoteMaterializationStore` | Whether to use remote gRPC for sticky assignments | false |

## License

Apache 2.0
