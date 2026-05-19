# Confidence Local Resolver — Swift (experimental)

Swift package that wraps the Confidence resolver compiled to WebAssembly and
runs it locally via [WasmKit](https://github.com/swiftwasm/WasmKit). Built as
part of the unit-local resolver experiment described in
[`UNIT_LOCAL_RESOLVER.md`](../../UNIT_LOCAL_RESOLVER.md). Not yet production
ready — the API will change.

## What's in here

| Target | Type | Purpose |
|--------|------|---------|
| `ConfidenceLocalResolver` | library | WASM runtime, `wasm-msg` message layer, slice HTTP client, typed resolver API |
| `LocalResolverCli` | executable | End-to-end driver: fetches a slice from the unit-local server, runs a resolve, prints results |

The WASM binary embedded in the library bundle is the same artifact the other
providers use (currently copied from
`openfeature-provider/go/.../confidence_resolver.wasm`).

## Quick start

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

Sample output:

```
[cli] account_id=confidence-demo-june state_file_hash=v1 units=["visitor_id"] full_state=245717 bytes
[cli] fetching slice for unit=user_42…
[cli] slice_bytes=4935  ratio=2.01%
[cli] resolved_flags=3 resolve_token_size=0
  - flag=flags/fallthrough-test-1 reason=match variant=…/enabled value={ enabled: true }
  - flag=flags/fallthrough-test-2 reason=match variant=…/enabled value={ enabled: true }
  - flag=flags/tutorial-feature reason=noSegmentMatch value={}
```

Override defaults via env vars:

- `UNIT_LOCAL_SERVER` — base URL of the slice server (default `http://127.0.0.1:8787`)
- `CONFIDENCE_CLIENT_SECRET` — client secret for the demo account

## Architecture

```
LocalResolverCli  ──► SliceClient ──HTTP──►  unit-local-server (Rust)
                                              ├─ loads test-payloads/resolver_state.pb once
                                              └─ slice_for_unit(state, account, unit) per request

LocalResolverCli  ──► LocalResolver
                       ├─ WasmKit Engine + Store
                       ├─ host imports: wasm_msg_host_{log_message,current_time}
                       └─ exports: wasm_msg_alloc/free, wasm_msg_guest_{set_resolver_state,resolve_flags,apply_flags}

WasmMsg              wire-format helpers (alloc → write → call → read → free)
                     wraps inner protobufs in Confidence_Wasm_{Request,Response}
```

## Regenerating proto bindings

```bash
brew install protobuf swift-protobuf  # one-off
./Tools/generate-protos.sh
```

Bindings live in `Sources/ConfidenceLocalResolver/Generated/`. They're checked
in to keep `swift build` standalone.
