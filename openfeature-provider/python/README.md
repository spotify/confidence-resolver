# Confidence OpenFeature Provider for Python

This is the Python OpenFeature provider for [Confidence](https://confidence.spotify.com/) feature flags. It uses a WebAssembly-based local resolver for low-latency flag evaluation.

## Installation

```bash
pip install confidence-openfeature-provider
```

## Usage

```python
from openfeature import api
from confidence_openfeature import ConfidenceProvider

# Create and register the provider
provider = ConfidenceProvider(client_secret="your-client-secret")
api.set_provider(provider)

# Get the client and evaluate flags
client = api.get_client()
result = client.get_boolean_value("my-flag.enabled", default_value=False)
```

## Features

- Local WASM-based flag resolution for low latency
- Automatic state polling for flag updates
- Async flag logging to Confidence backend
- Support for materialized (sticky) assignments

## Configuration

```python
provider = ConfidenceProvider(
    client_secret="your-client-secret",
    state_poll_interval=30.0,  # How often to poll for state updates (seconds)
    log_poll_interval=10.0,    # How often to flush logs (seconds)
)
```

## License

Apache 2.0
