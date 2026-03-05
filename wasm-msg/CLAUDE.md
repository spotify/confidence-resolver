# wasm-msg — WASM Messaging Layer

## Overview

Crate: `wasm-msg` (internal, `publish = false`)

Provides the message-passing protocol between WASM guest (Rust resolver) and host languages. Solves the fundamental WASM interop problem: WASM functions can only pass integers/pointers, but the resolver needs to exchange protobuf messages.

## Memory Convention

All allocations store the total allocation size (as `usize`) immediately before the data pointer:

```
[total_size: usize][data: u8 * N]
                   ^-- returned pointer
```

- `wasm_msg_alloc(size)` → allocates `size + sizeof(usize)`, returns pointer to data area
- `wasm_msg_free(ptr)` → reads size from `ptr - sizeof(usize)`, deallocates

## Message Protocol

Every guest↔host call goes through protobuf envelope types (defined in `proto/messages.proto`):

- **Request** → `{ data: bytes }` — wraps the serialized request protobuf
- **Response** → `{ oneof result { data: bytes, error: string } }` — wraps result or error

Flow for a guest function call:
1. Host serializes `Request{data: <inner request bytes>}` into WASM memory via `wasm_msg_alloc`
2. Host calls `wasm_msg_guest_<name>(ptr) -> ptr`
3. Guest reads `Request`, decodes inner type, runs handler, returns `Response` via `transfer_response`
4. Host reads `Response`, extracts data or error, frees memory

## Macros

### `wasm_msg_guest!`

Generates both the handler function and the `#[no_mangle] extern "C"` WASM export:

```rust
wasm_msg_guest! {
    fn my_func(request: MyRequest) -> WasmResult<MyResponse> {
        Ok(MyResponse { ... })
    }
}
// Generates: pub extern "C" fn wasm_msg_guest_my_func(ptr: *mut u8) -> *mut u8
```

### `wasm_msg_host!`

Generates FFI import declarations and safe wrapper functions:

```rust
wasm_msg_host! {
    fn log_message(message: LogMessage) -> WasmResult<Void>;
}
// Generates: extern "C" { fn wasm_msg_host_log_message(ptr: *mut u8) -> *mut u8; }
// Plus: pub fn log_message(message: LogMessage) -> WasmResult<Void>
```

Host imports use the WASM import module `"wasm_msg"`.

## Build & Test

```bash
make lint   # cargo fmt --check + cargo clippy --lib --release -- -D warnings
make test   # no tests yet (all resolver logic tested in confidence-resolver)
```

## Proto Generation

`build.rs` uses `tonic-build` to compile `proto/messages.proto` (unusual — `tonic-build` is used here even though this isn't a gRPC crate).
