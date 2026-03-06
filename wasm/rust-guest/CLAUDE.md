# WASM Guest — Rust-to-WASM Build

## Overview

The `rust-guest` crate compiles the Confidence resolver to WebAssembly (`wasm32-unknown-unknown`). It is the bridge between host languages (JS, Java, Go, Python, Ruby) and the core resolver.

**Note**: This crate uses `std` (not `no_std`). It uses `std::sync::Arc`, `std::sync::LazyLock`, and the standard allocator — even though it targets WASM.

## Source

The entire crate is a single file: `src/lib.rs`. It:
1. Manages global state via `static` items (`RESOLVER_STATE`, `RESOLVE_LOGGER`, `ASSIGN_LOGGER`, `TELEMETRY`)
2. Implements the `Host` trait for `WasmHost` (logging, time, resolve/assign event dispatch)
3. Declares guest functions via `wasm_msg_guest!` macro
4. Declares host imports via `wasm_msg_host!` macro

`confidence_resolver` is included with `default-features = false` — disabling the `std` and `json` features in the resolver (but the guest crate itself uses `std`).

## WASM Exports

The `wasm_msg_guest!` macro generates exported functions with the `wasm_msg_guest_` prefix. Each takes a `*mut u8` (pointer to protobuf-encoded request) and returns `*mut u8` (pointer to protobuf-encoded response).

| Export Name | Request Type | Response Type | Purpose |
|-------------|-------------|--------------|---------|
| `wasm_msg_guest_init_thread` | `InitThreadRequest` | `Void` | Seed RNG for current thread |
| `wasm_msg_guest_set_resolver_state` | `SetResolverStateRequest` | `Void` | Load resolver state (protobuf-encoded) |
| `wasm_msg_guest_resolve_flags` | `ResolveProcessRequest` | `ResolveProcessResponse` | Resolve flags |
| `wasm_msg_guest_flush_logs` | `Void` | `WriteFlagLogsRequest` | Flush resolve + assign logs (deprecated) |
| `wasm_msg_guest_bounded_flush_logs` | `Void` | `WriteFlagLogsRequest` | Flush logs with telemetry delta |
| `wasm_msg_guest_bounded_flush_assign` | `Void` | `WriteFlagLogsRequest` | Flush assign logs only (bounded) |
| `wasm_msg_guest_apply_flags` | `ApplyFlagsRequest` | `Void` | Apply flags (best-effort logging) |

Memory management exports (from `wasm-msg`):
- `wasm_msg_alloc(size: usize) -> *mut u8` — Allocate memory (stores size prefix before data)
- `wasm_msg_free(ptr: *mut u8)` — Free memory (reads size prefix to determine layout)

Host imports (via `wasm_msg_host!` macro, import module `wasm_msg`):
- `wasm_msg_host_log_message` — Send log message to host
- `wasm_msg_host_current_time` — Get current time from host

## Proto Files

Proto definitions are at `wasm/proto/` (NOT `wasm/rust-guest/proto/` — no such directory exists). The `build.rs` compiles `messages.proto` and `types.proto` from `../proto` relative to the crate root.

## Build

```bash
make build  # cargo build --target wasm32-unknown-unknown --profile wasm
make lint   # cargo fmt --check + cargo clippy --target wasm32-unknown-unknown
```

No `test` target — all resolver logic is tested in the `confidence-resolver` crate. This crate is a thin WASM wrapper.

Typical optimized size: **~450 KB**.
