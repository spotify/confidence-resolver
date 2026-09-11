# Confidence OpenFeature Provider for Python

![Status: Alpha](https://img.shields.io/badge/status-alpha-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using WebAssembly (WASM)
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging (and other resolve analytics)
- **[Event Tracking](#event-tracking)**: Send custom events via the OpenFeature `track()` API
- **OpenFeature Compatible**: Works with the standard OpenFeature SDK

## Requirements

- Python 3.10+
- OpenFeature SDK 0.10.0+

## Installation

```bash
pip install confidence-openfeature-provider
```

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:
- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

## Encryption

The provider fetches encrypted flag state and decrypts it when loading the resolver. Pass the encryption key for your client credential when creating the provider.

See the [Integration Guide](../INTEGRATION_GUIDE.md#encryption) for background and migration details.

Pass the encryption key when creating the provider:

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",  # Required; unique to this credential
)
```

Each client credential has a unique encryption key, available alongside it in [Confidence Admin](https://app.confidence.spotify.com/admin/clients).

## Quick Start

```python
from openfeature import api
from openfeature.evaluation_context import EvaluationContext
from confidence import ConfidenceProvider

# Create and register the provider
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
)
api.set_provider_and_wait(provider)

# Get a client
client = api.get_client()

# Create evaluation context with user attributes for targeting
context = EvaluationContext(
    targeting_key="user-123",
    attributes={
        "country": "US",
        "plan": "premium",
    }
)

# Evaluate a flag
enabled = client.get_boolean_value("test-flag.enabled", default_value=False, evaluation_context=context)
print(f"Flag value: {enabled}")

# Don't forget to shutdown when your application exits (see Shutdown section)
```

## Async Evaluation

In an asyncio application, use the OpenFeature client's async evaluation methods:

```python
# After registering the provider, as shown above:
async def feature_enabled() -> bool:
    return await client.get_boolean_value_async(
        "test-flag.enabled", default_value=False, evaluation_context=context
    )
```

Async evaluation is supported for boolean, string, integer, float, and object flags.
The provider runs evaluation in a worker thread, so waiting for the WASM resolver
or materialization storage leaves the event loop free to handle other requests.
Evaluations still share one WASM instance protected by a lock.

Provider initialization and shutdown remain synchronous; perform them outside
the event loop or offload them with `asyncio.to_thread`. Cancelling an async
evaluation does not stop work already running in its worker thread.

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### Python Examples

```python
from openfeature.evaluation_context import EvaluationContext

# Simple attributes
context = EvaluationContext(
    targeting_key="user-123",
    attributes={
        "country": "US",
        "plan": "premium",
        "age": 25,
    }
)
```

## Error Handling

The provider uses a **default value fallback** pattern - when evaluation fails, it returns your specified default value instead of throwing an error.

**📖 See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:
- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### Python Examples

```python
# The provider returns the default value on errors
enabled = client.get_boolean_value("my-flag.enabled", default_value=False, evaluation_context=context)
# enabled will be False if evaluation failed

# For detailed error information, use get_boolean_details()
details = client.get_boolean_details("my-flag.enabled", default_value=False, evaluation_context=context)
if details.error_code:
    print(f"Flag evaluation error: {details.error_message}")
    print(f"Reason: {details.reason}")
```

## Shutdown

**Important**: To ensure proper cleanup and flushing of exposure logs, you should call `shutdown()` on the provider when your application exits.

```python
from openfeature import api
# Shutdown the provider to flush logs and clean up resources
api.shutdown()
```

## Configuration

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
    state_poll_interval=30.0,  # How often to poll for state updates (seconds)
    log_poll_interval=10.0,    # How often to flush logs (seconds)
)
```

### Configuration Options

- `client_secret` (str, required): The Confidence client secret for authentication.
- `encryption_key` (str, required): Encryption key for decrypting the flag state. Found in the [Confidence Admin view](https://app.confidence.spotify.com/admin/clients).
- `state_poll_interval` (float, optional): Interval in seconds between state polling updates. Defaults to 30.0.
- `log_poll_interval` (float, optional): Interval in seconds for sending evaluation logs. Defaults to 10.0.
- `use_remote_materialization_store` (bool, optional): Enable remote materialization storage. Defaults to False.
- `grpc_channel` (grpc.Channel, optional): Custom gRPC channel for flag log shipping. When not provided, the default channel retries flag log writes on transient failures (3 attempts with exponential backoff on `UNAVAILABLE`). If you provide your own channel, configure retry via [gRPC service config](https://grpc.io/docs/guides/retry/) to get the same behavior.

## Apply-event deduplication

Apply-event deduplication is **on by default**. The WASM resolver collapses
byte-identical apply events for the same unit and variant within a 120-second
TTL window (tracking up to 100,000 entries), so repeatedly resolving the same
flag for the same targeting key produces one apply event instead of many.

Upgrading without changing any code therefore reduces raw apply-event volume,
substantially so for high-QPS traffic that resolves the same flag for the same
unit. Distinct units are never collapsed, because the targeting key is part of
the dedup key, so per-unit exposure counts are unaffected.

To turn it off and log every apply:

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
    enable_apply_dedup=False,
)
```

## Materializations

The provider supports **materializations** for two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change.
2. **Custom Targeting via Materialized Segments**: Efficiently target precomputed sets of identifiers from datasets.

### Default Behavior

By default, materializations are not supported. If a flag requires materialization data, the evaluation will return the default value.

### Remote Materialization Store

Enable remote materialization storage to have Confidence manage materialization data server-side:

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
    use_remote_materialization_store=True,
)
```

**⚠️ Important Performance Impact**: This option adds network calls during flag evaluation for materialization reads/writes.

### Custom Materialization Store

For advanced use cases, you can implement the `MaterializationStore` protocol to manage materialization data in your own infrastructure. The protocol defines two methods:

- `read(ops: List[ReadOp]) -> List[ReadResult]`: Batch read of materialization data
- `write(ops: List[VariantWriteOp]) -> None`: Batch write of variant assignments

The read operations support two types:

- **VariantReadOp**: Query for a sticky variant assignment (returns `VariantReadResult`)
- **InclusionReadOp**: Query for segment inclusion (returns `InclusionReadResult`)

```python
from confidence.materialization import (
    MaterializationStore,
    ReadOp,
    ReadResult,
    VariantReadOp,
    VariantReadResult,
    InclusionReadOp,
    InclusionReadResult,
    VariantWriteOp,
)

class MyMaterializationStore:
    """Custom materialization store implementation."""

    def read(self, ops: list[ReadOp]) -> list[ReadResult]:
        results = []
        for op in ops:
            if isinstance(op, VariantReadOp):
                # Look up sticky variant assignment
                variant = self._lookup_variant(op.unit, op.materialization, op.rule)
                results.append(VariantReadResult(
                    unit=op.unit,
                    materialization=op.materialization,
                    rule=op.rule,
                    variant=variant,  # None if no assignment exists
                ))
            elif isinstance(op, InclusionReadOp):
                # Check segment inclusion
                included = self._check_inclusion(op.unit, op.materialization)
                results.append(InclusionReadResult(
                    unit=op.unit,
                    materialization=op.materialization,
                    included=included,
                ))
        return results

    def write(self, ops: list[VariantWriteOp]) -> None:
        for op in ops:
            # Store sticky variant assignment
            self._store_variant(op.unit, op.materialization, op.rule, op.variant)
```

Pass your custom store to the provider:

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
    materialization_store=MyMaterializationStore(),
)
```

**Thread Safety**: Your implementation must be thread-safe as it may be called concurrently from multiple threads.

## Logging

Configure logging to see provider activity:

```python
import logging
logging.getLogger("confidence").setLevel(logging.DEBUG)
```

## Event Tracking

The provider supports the [OpenFeature tracking API](https://openfeature.dev/specification/sections/tracking) for sending custom events to the Confidence events backend. Event tracking is automatically enabled — no configuration needed.

**📖 See the [Integration Guide: Event Tracking](../INTEGRATION_GUIDE.md#event-tracking)** for delivery guarantees, payload mapping rules, and cross-provider differences.

### Usage

```python
client = api.get_client()

context = EvaluationContext(
    targeting_key="user-123",
    attributes={"country": "US"},
)

# Track a simple event
client.track("checkout_completed", context)

# Track with a numeric value
client.track("purchase", context, TrackingEventDetails(value=49.99))

# Track with custom data
client.track("item_added", context, TrackingEventDetails(
    value=1,
    attributes={"sku": "ABC-123", "category": "electronics"},
))
```

Events are batched internally and flushed to the Confidence events service at the same interval as flag logs (configurable via `log_poll_interval`). On shutdown, pending events are drained on a best-effort basis (up to 100 batches).

## Advanced: Controlling Exposure Events

By default, every flag evaluation records an exposure event (apply). Only disable this for exceptional cases where this provider must not collect exposures at all.

For normal feature delivery and experiments, keep applies enabled. When exposure collection is disabled, Confidence does not receive assignment/exposure events for those evaluations. Experiment results, exposure counts, assignment diagnostics, and downstream reporting that depend on exposures can be incomplete or unavailable. Resolve analytics and telemetry are still sent, so this is not a general logging or privacy-off switch.

To disable exposure collection for **all** OpenFeature evaluations through this provider, pass `disable_exposure_collection=True` when constructing the provider:

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    encryption_key="your-encryption-key",
    disable_exposure_collection=True,
)
```

To skip exposure collection for a single evaluation, pass `_confidence_skip_apply` in the evaluation context:

```python
context = EvaluationContext(
    targeting_key="user-123",
    attributes={"_confidence_skip_apply": True},
)

value = client.get_boolean_value("my-flag.enabled", False, context)
```

The key is automatically stripped from the context before it reaches the resolver.

| Mechanism | Scope | Assignment/exposure events | Resolve logs and telemetry |
| --- | --- | --- | --- |
| `disable_exposure_collection` provider option | All OpenFeature evaluations through this provider | Never queued; no deferred apply token is returned | Still sent |
| `_confidence_skip_apply` context key | One evaluation | No immediate exposure event for that evaluation | Still sent |

This is an advanced feature intended for exceptional cases. If you're considering using it, reach out to the Confidence team to discuss the best approach for your setup.

## License

Apache 2.0

### Migrating to mandatory encryption

`encryption_key` is now a required constructor argument. Missing, empty, or invalid
keys fail before initialization. Supply exactly 64 hexadecimal characters; the
provider only fetches encrypted state and never falls back to plaintext.

Open [Confidence Admin → Clients](https://app.confidence.spotify.com/admin/clients),
select your client, and find the credential used by the provider. Each credential
has its own unique encryption key, available alongside it. Use the key paired with
your configured client secret. Configure and verify encryption on your existing
SDK before upgrading.
