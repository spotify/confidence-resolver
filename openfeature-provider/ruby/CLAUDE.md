# Ruby OpenFeature Provider

## Overview

Gem: `confidence-openfeature-provider`

Ruby OpenFeature provider for Confidence. **Unlike other providers, this is an online/remote resolver** — it makes HTTP API calls to the Confidence backend for each flag evaluation rather than using WASM for local resolution.

## Architecture

- **No WASM** — This provider calls the Confidence resolve API over HTTP using `Net::HTTP` via `ApiClient`
- **Synchronous** — All flag evaluations are blocking HTTP calls
- **Apply on resolve** — By default, flags are applied (logged) during resolution (`apply_on_resolve: true`)

## Build & Test

```bash
make build    # bundle exec rake build
make test     # bundle exec rake spec
make lint     # bundle exec rake standard
make install  # bundle install
```
