# Confidence OpenFeature Provider for Python

![Status: Alpha](https://img.shields.io/badge/status-alpha-orange)

A high-performance OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags that evaluates flags locally for minimal latency.

## Features

- **Local Resolution**: Evaluates feature flags locally using WebAssembly (WASM)
- **Low Latency**: No network calls during flag evaluation
- **Automatic Sync**: Periodically syncs flag configurations from Confidence
- **Exposure Logging**: Fully supported exposure logging (and other resolve analytics)
- **OpenFeature Compatible**: Works with the standard OpenFeature SDK

## Requirements

- Python 3.9+
- OpenFeature SDK 0.8.0+

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

## Quick Start

```python
from openfeature import api
from openfeature.evaluation_context import EvaluationContext
from confidence import ConfidenceProvider

# Create and register the provider
provider = ConfidenceProvider(client_secret="your-client-secret")
api.set_provider(provider)

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
    state_poll_interval=30.0,  # How often to poll for state updates (seconds)
    log_poll_interval=10.0,    # How often to flush logs (seconds)
)
```

### Configuration Options

- `client_secret` (str, required): The Confidence client secret for authentication.
- `state_poll_interval` (float, optional): Interval in seconds between state polling updates. Defaults to 30.0.
- `log_poll_interval` (float, optional): Interval in seconds for sending evaluation logs. Defaults to 10.0.
- `use_remote_materialization_store` (bool, optional): Enable remote materialization storage. Defaults to False.

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

## License

Apache 2.0
