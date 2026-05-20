# Confidence Local Provider — Swift (experimental)

Swift package implementing an [OpenFeature](https://openfeature.dev/) provider
that resolves Confidence flags locally on iOS / macOS using the Confidence
resolver compiled to WebAssembly. Runs the WASM via
[WasmKit](https://github.com/swiftwasm/WasmKit). Built as part of the
unit-local resolver experiment described in
[`UNIT_LOCAL_RESOLVER.md`](../../UNIT_LOCAL_RESOLVER.md). Not yet
production-ready — the API will change.

## What's in here

| Target | Type | Purpose |
|--------|------|---------|
| `ConfidenceLocalResolver` | library | WASM runtime, host-side `wasm-msg` message layer, slice HTTP client, typed resolver API |
| `ConfidenceLocalProvider` | library | `OpenFeature.FeatureProvider` implementation on top of `ConfidenceLocalResolver`. Flag cache, unit-change re-slicing, apply round-trip. |
| `LocalResolverCli` | executable | End-to-end driver exercising the lower layer directly (without OpenFeature). |
| `ConfidenceProtoGen` | build plugin | Generates `*.pb.swift` from the shared `openfeature-provider/proto/` (symlinked under `Sources/ConfidenceLocalResolver/Protos`) at build time. Requires `protoc` on `PATH` (Apple Silicon brew default is auto-detected). |

The WASM binary embedded in the library bundle is the same artifact the other
providers use (currently copied from
`openfeature-provider/go/.../confidence_resolver.wasm`).

## Quick start (CLI)

In one terminal, run the local slice server (from the repo root):

```bash
cargo run -p unit-local-server
# listening on http://127.0.0.1:8787
```

In another terminal, run the Swift CLI for a chosen randomization unit:

```bash
cd openfeature-provider/swift
swift run LocalResolverCli user_42
```

## Quick start (iOS simulator demo)

```bash
cargo run -p unit-local-server
open openfeature-provider/swift/DemoApp/DemoApp.xcodeproj
# Run on any iOS 16+ simulator. The app calls OpenFeatureAPI.shared.getDetails(...)
# against ConfidenceLocalProvider; on every resolve the server logs the slice
# size + an apply round-trip.
```

## Architecture

```
DemoApp (SwiftUI iOS)
    │
    ▼
OpenFeatureAPI.shared.setProvider(ConfidenceLocalProvider(...))
    │
    ▼
ConfidenceLocalProvider                ── implements FeatureProvider
    ├─ flag cache (per resolve)
    ├─ unit-change detection           ── triggers slice refetch
    └─ uses ─►
        SliceClient ──HTTP──►  unit-local-server (Rust)
                                  ├─ /v1/account-config
                                  ├─ /v1/resolver-state/{hash}/{unit}
                                  └─ /v1/apply
        LocalResolver
            ├─ WasmKit Engine + Store
            ├─ host imports: wasm_msg_host_{log_message,current_time}
            └─ exports: wasm_msg_alloc/free, wasm_msg_guest_{set_resolver_state,resolve_flags,apply_flags}

WasmMsg          wire-format helpers (alloc → write → call → read → free)
                 wraps inner protobufs in Confidence_Wasm_{Request,Response}
```

## Proto codegen

`Sources/ConfidenceLocalResolver/Protos` is a symlink to the shared
`openfeature-provider/proto/` directory. The `ConfidenceProtoGen` build-tool
plugin runs `protoc` + `protoc-gen-swift` at build time with
`FileNaming=PathToUnderscores`, producing the six `.pb.swift` files under
`.build/.../ConfidenceProtoGen/Generated/`. Nothing is checked in.

Requirements:

```bash
brew install protobuf   # provides /opt/homebrew/bin/protoc
```

If `protoc` lives elsewhere, set `PROTOC_PATH=/usr/local/bin/protoc` (etc.)
before building.
