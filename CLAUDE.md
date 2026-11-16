# Confidence Providers (`next`) - Development Guide

## Repository Overview

**Repository**: `spotify/confidence-resolver`. This is the long-lived `next` branch.

`next` moves the OpenFeature providers from the in-repo Rust resolver (rust-guest wasm) to
state-to-wasm: each credential's flags are compiled to a WebAssembly module, served from a CDN, and
run by the provider in a wasm runtime. The resolver itself lives in state-to-wasm, not here.

- Every change lands on `next` as a reviewed PR. Nothing is merged from `main`; provider fixes made
  on `main` after the branch point are reviewed when that provider is rewritten.
- Providers are rewritten one at a time. Until then a provider is **API reference only**: its
  source stays, it doesn't build, and its README says so.
- Rewritten providers are released as release candidates from `next`.
- When all providers are done, `next` replaces `main` in one merge.

### Key Components

- **openfeature-provider/<lang>/** — the providers (Go, Java, JS, Python, Ruby, Rust)
- **openfeature-provider/proto/** — verbatim copies of the state-to-wasm protos providers use; see
  its README for the source commit and how to update
- **confidence-cloudflare-resolver/** — Cloudflare Worker resolver (reference, pending rewrite)

## Build & Run

```bash
make           # lint + test + build every provider in the root Makefile's PROVIDERS
docker build . # the same in Docker; this is CI
```

A rewritten provider adds itself to `PROVIDERS` in the root `Makefile` and adds its Docker stages.

### Docker

The Dockerfile uses multi-stage builds. Every provider has `.<action>` stages named
`<component>.{build,test,test_e2e,lint,artifact,publish}` (e.g. `openfeature-provider-go.test`),
and wires the ones CI runs into the `all` stage (the default target; no secrets) or, for
`test_e2e`, into the `e2e` stage (secrets mounted).

## Publishing & Security

### Critical: Secrets Must Never Be Written to Docker Layers

```dockerfile
# CORRECT — secret mounted, never persisted
RUN --mount=type=secret,id=my_secret,target=/path/to/secret \
    command-that-uses /path/to/secret

# WRONG — secret written to layer
RUN --mount=type=secret,id=my_secret \
    cat /run/secrets/my_secret > config.file
```

Releases are managed by Release Please; see `.github/workflows/CLAUDE.md`.

## Post-Release Checklist

After releasing a new version of any SDK from this repo, update internal version registries with the new version numbers.
