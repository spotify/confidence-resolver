# WASM Build - Rust Guest Development Guide

This guide covers building the Confidence resolver as a WebAssembly module.

## Overview

The `rust-guest` crate compiles the Confidence resolver to WebAssembly (WASM), enabling it to run in JavaScript, Java, Go, Python, and other host environments.

**Key features:**
- Cross-platform distribution via WASM
- Message-based API using Protocol Buffers
- Optimized for size (~400-600 KB)
- No_std compatible for minimal runtime

## Project Structure

```
wasm/rust-guest/
├── src/
│   ├── lib.rs                 # WASM entry points
│   └── *.rs                   # WASM-specific code
├── proto/
│   └── *.proto                # WASM message definitions
├── build.rs                   # Build script (proto generation)
├── Cargo.toml                 # WASM-specific dependencies
├── Makefile                   # Build automation
└── CLAUDE.md                  # This file
```

## Build Target

The crate compiles to the `wasm32-unknown-unknown` target:

- **No operating system**: Bare WASM without WASI
- **No standard library dependencies**: Uses `no_std` where possible
- **Small binary size**: Optimized for download and execution speed

## Dependencies

### Production Dependencies

```toml
[dependencies]
confidence-resolver = { path = "../../confidence-resolver" }
wasm-msg = { path = "../../wasm-msg" }
prost = "..."           # Protocol Buffer runtime
```

### WASM-Specific Configuration

```toml
[lib]
crate-type = ["cdylib"]  # Compile as C dynamic library (for WASM)

[profile.wasm]
inherits = "release"
opt-level = "z"          # Optimize for size
lto = true               # Link-time optimization
codegen-units = 1        # Better optimization
strip = "symbols"        # Remove debug symbols
```

## Local Development

### Prerequisites

```bash
# Rust with WASM target
rustup target add wasm32-unknown-unknown

# Protocol Buffers compiler
brew install protobuf  # macOS
# or
apt-get install protobuf-compiler  # Ubuntu

# WASM tools (optional, for inspection)
cargo install wasm-objdump
cargo install wasm-opt
```

### Setup

```bash
cd wasm/rust-guest

# Add WASM target
rustup target add wasm32-unknown-unknown

# Build
cargo build --target wasm32-unknown-unknown --profile wasm
```

## Building

### Debug Build

```bash
# Build WASM in debug mode (larger, with debug info)
cargo build --target wasm32-unknown-unknown
# Output: ../../target/wasm32-unknown-unknown/debug/rust_guest.wasm
```

### Release Build (Optimized)

```bash
# Build with wasm profile (size-optimized)
cargo build --target wasm32-unknown-unknown --profile wasm
# Output: ../../target/wasm32-unknown-unknown/wasm/rust_guest.wasm

# Using Make
make build
```

### Build Output Location

```bash
# From wasm/rust-guest directory
../../target/wasm32-unknown-unknown/wasm/rust_guest.wasm

# Canonical location (copied by root Makefile)
../../wasm/confidence_resolver.wasm
```

### Using Docker

```bash
# Build from repo root
docker build --target wasm-rust-guest.build .

# Extract artifact
docker build --target wasm-rust-guest.artifact -o wasm .
```

## Size Optimization

### Current Size

Typical optimized build: **~400-600 KB**

### Optimization Techniques Used

1. **Profile Configuration**:
   ```toml
   [profile.wasm]
   opt-level = "z"        # Optimize for size
   lto = true             # Link-time optimization
   codegen-units = 1      # Better optimization (slower build)
   strip = "symbols"      # Remove debug symbols
   ```

2. **Crate Type**:
   ```toml
   [lib]
   crate-type = ["cdylib"]  # Dynamic library
   ```

3. **Minimal Dependencies**:
   - Avoid large dependencies
   - Use `no_std` where possible
   - Feature-gate optional functionality

4. **Dead Code Elimination**:
   - LTO removes unused code
   - Mark internal functions `#[inline]`
   - Use `#[link_section]` for cold paths

### Further Optimization

```bash
# Post-process with wasm-opt (from binaryen)
brew install binaryen
wasm-opt -Oz input.wasm -o output.wasm

# Analyze size
wasm-objdump -h rust_guest.wasm
```

### Size Benchmarking

```bash
# Check size
ls -lh ../../target/wasm32-unknown-unknown/wasm/rust_guest.wasm

# Compare debug vs release
ls -lh ../../target/wasm32-unknown-unknown/debug/rust_guest.wasm
ls -lh ../../target/wasm32-unknown-unknown/wasm/rust_guest.wasm
```

## WASM API

### Message-Based Interface

The WASM module exposes a Protocol Buffer-based API:

```rust
#[no_mangle]
pub extern "C" fn resolve(input_ptr: *const u8, input_len: usize) -> i64 {
    // 1. Decode protobuf message from memory
    // 2. Call resolver
    // 3. Encode response as protobuf
    // 4. Return pointer to result
}
```

### Memory Management

```rust
// Allocate memory for host to write input
#[no_mangle]
pub extern "C" fn alloc(size: usize) -> *mut u8 { ... }

// Free memory allocated by WASM
#[no_mangle]
pub extern "C" fn dealloc(ptr: *mut u8, size: usize) { ... }
```

## Testing

### Unit Tests

```bash
# Run tests (native target, not WASM)
cargo test
```

Note: Tests run on the host architecture, not in WASM. For WASM-specific testing, use host integration tests.

### Integration Tests

Integration tests are in the host examples:

```bash
# From repo root
make integration-test

# Or specific host
cd ../node-host && yarn start
cd ../java-host && mvn exec:java
cd ../go-host && go run .
cd ../python-host && python resolver_api.py
```

## Linting

### Clippy for WASM Target

```bash
# Run clippy for WASM target
cargo clippy --target wasm32-unknown-unknown

# Using Make
make lint
```

### Common WASM-Specific Lints

- Avoid `std::` in WASM context
- Check pointer safety
- Validate memory allocations
- Ensure proper error handling

## Inspecting WASM

### Using wasm-objdump

```bash
# Install
cargo install wasm-objdump

# Show sections
wasm-objdump -h rust_guest.wasm

# Show exports
wasm-objdump -x rust_guest.wasm | grep export

# Disassemble
wasm-objdump -d rust_guest.wasm
```

### Analyzing Size

```bash
# Section sizes
wasm-objdump -h rust_guest.wasm

# Detailed analysis
wasm-objdump -s rust_guest.wasm
```

## Common Tasks

### Update Resolver Dependency

```bash
# confidence-resolver is in workspace
# Changes are automatically picked up

# Rebuild WASM
cargo build --target wasm32-unknown-unknown --profile wasm
```

### Add New WASM Export

```rust
#[no_mangle]
pub extern "C" fn new_function(arg: i32) -> i32 {
    // Implementation
}
```

### Debug WASM Module

```bash
# Build with debug info
cargo build --target wasm32-unknown-unknown

# The WASM binary is larger but includes debug info
# Use browser devtools or wasmtime for debugging
```

### Verify ABI Compatibility

```bash
# Check exports match expected interface
wasm-objdump -x rust_guest.wasm | grep export

# Should see:
# - alloc
# - dealloc
# - resolve
# (and any other public functions)
```

## Troubleshooting

### "linker 'rust-lld' not found"

**Solution**: Install WASM target:
```bash
rustup target add wasm32-unknown-unknown
```

### Large binary size

**Solution**: Ensure using `--profile wasm`:
```bash
cargo build --target wasm32-unknown-unknown --profile wasm
```

### Proto generation fails

**Solution**: Install protoc:
```bash
brew install protobuf  # macOS
apt-get install protobuf-compiler  # Ubuntu
```

### Host can't load WASM

**Solution**: Verify export names:
```bash
wasm-objdump -x rust_guest.wasm | grep export
```

### Memory issues in WASM

**Solution**: Check allocator configuration and memory limits in host.

## Best Practices

### Size Optimization

- Keep dependencies minimal
- Use `no_std` where possible
- Feature-gate optional functionality
- Profile and measure size impact
- Use LTO and opt-level = "z"

### API Design

- Use Protocol Buffers for all interfaces
- Return error codes, not panics
- Document memory ownership
- Version your API

### Error Handling

```rust
// Don't panic in WASM!
// Instead, return error codes

#[no_mangle]
pub extern "C" fn resolve(...) -> i64 {
    match do_resolve() {
        Ok(result) => encode_success(result),
        Err(e) => encode_error(e),
    }
}
```

### Testing

- Test on native target for unit tests
- Use host integration tests for WASM validation
- Test across different host languages
- Verify memory safety

## Publishing

WASM artifacts are published to GitHub Releases:

```bash
# Automated via GitHub Actions
# Manual extraction:
docker build --target wasm-rust-guest.artifact -o wasm ../..
```

See [root CLAUDE.md](../../CLAUDE.md#wasm-artifact-publishing) for details.

## Host Integration

The WASM module is used by:

- **Node.js** (`../node-host`)
- **Java** (`../java-host`)
- **Go** (`../go-host`)
- **Python** (`../python-host`)

Each host implements the Protocol Buffer messaging protocol to communicate with WASM.

## Performance Considerations

### Startup Time

WASM modules have fast instantiation:
- Small module size = faster download
- No JIT compilation
- Deterministic startup

### Runtime Performance

- WASM runs at near-native speed
- JIT-compiled by browser/runtime
- No garbage collection overhead
- Predictable performance

### Memory Usage

- Linear memory model
- No heap fragmentation
- Configurable memory limits
- Efficient for embedded use

## Additional Resources

- **[Repository root CLAUDE.md](../../CLAUDE.md)** - Overall development guide
- **[confidence-resolver/CLAUDE.md](../../confidence-resolver/CLAUDE.md)** - Core library development
- **WebAssembly Docs**: https://webassembly.org/
- **Rust WASM Book**: https://rustwasm.github.io/docs/book/
- **wasm-bindgen**: https://rustwasm.github.io/wasm-bindgen/
- **MDN WebAssembly**: https://developer.mozilla.org/en-US/docs/WebAssembly
