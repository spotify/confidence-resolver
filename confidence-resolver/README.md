# confidence_resolver

Core feature flag resolution library for [Confidence](https://confidence.spotify.com/).

This crate provides the fundamental flag resolution logic used by the various SDK implementations in this monorepo. It handles flag evaluation, targeting rules, and value resolution.

## Usage

This library is typically consumed through one of the higher-level SDK integrations (e.g., the OpenFeature provider) rather than directly.

## Features

- `std` (default) - Enables standard library support
- `json` - Enables JSON serialization via serde

## License

Apache-2.0
