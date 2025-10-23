# Confidence Resolver - Core Rust Library Development Guide

This guide covers development of the core Confidence resolver library written in Rust.

## Overview

The `confidence-resolver` crate is the core library that implements feature flag resolution logic. It's compiled to both native Rust and WebAssembly, enabling use across multiple platforms.

**Key responsibilities:**
- Parse and validate resolver state
- Resolve feature flags based on targeting rules
- Evaluate flag values with type safety
- Handle bucketing and variant assignment

## Project Structure

```
confidence-resolver/
├── src/
│   ├── lib.rs                 # Library entry point
│   └── *.rs                   # Core implementation
├── protos/
│   └── *.proto                # Protocol Buffer definitions
├── build.rs                   # Build script (proto generation)
├── Cargo.toml                 # Dependencies and metadata
├── Makefile                   # Build automation
└── CLAUDE.md                  # This file
```

## Dependencies

### Production Dependencies

```toml
[dependencies]
prost = "..."           # Protocol Buffer runtime
serde = "..."           # Serialization
# ... see Cargo.toml for full list
```

### Build Dependencies

```toml
[build-dependencies]
prost-build = "..."     # Proto compilation at build time
```

## Local Development

### Prerequisites

```bash
# Rust toolchain (auto-installed from rust-toolchain.toml in repo root)
rustup show

# Protocol Buffers compiler
brew install protobuf  # macOS
# or
apt-get install protobuf-compiler  # Ubuntu
```

### Setup

```bash
cd confidence-resolver

# Build
cargo build

# Run tests
cargo test

# Run clippy
cargo clippy
```

### Development Workflow

```bash
# Check compilation
cargo check

# Build in debug mode
cargo build

# Build in release mode
cargo build --release

# Run tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture

# Run clippy (linter)
cargo clippy

# Run formatter check
cargo fmt --check

# Apply formatting
cargo fmt

# Build documentation
cargo doc --open
```

## Testing

### Running Tests

```bash
# All tests
cargo test

# All tests (via Makefile)
make test

# Specific test
cargo test resolve_flag

# Test with logging
RUST_LOG=debug cargo test

# Test with backtrace
RUST_BACKTRACE=1 cargo test
```

### Test Organization

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature() {
        // Arrange
        let input = ...;

        // Act
        let result = function(input);

        // Assert
        assert_eq!(result, expected);
    }
}
```

### Testing Best Practices

- Use `#[cfg(test)]` for test modules
- One test per function being tested
- Use descriptive test names
- Test edge cases and error paths
- Use property-based testing where appropriate

## Building

### Debug Build

```bash
cargo build
# Output: target/debug/libconfidence_resolver.rlib
```

### Release Build

```bash
cargo build --release
# Output: target/release/libconfidence_resolver.rlib
```

### Using Make

```bash
# Build and test
make

# Just build
make build

# Just test
make test

# Lint
make lint

# Clean
cargo clean
```

## Protocol Buffers

### Proto Files

Located in `protos/`:
- Core API definitions
- Message formats
- Resolver state schema

### Generation

Protos are automatically compiled by `build.rs` when you run `cargo build`:

```rust
// build.rs
fn main() {
    prost_build::compile_protos(
        &["protos/api.proto"],
        &["protos/"],
    ).unwrap();
}
```

Generated code appears in `target/` and is imported via:

```rust
// In src files
mod proto {
    include!(concat!(env!("OUT_DIR"), "/confidence.v1.rs"));
}
```

### Updating Protos

```bash
# 1. Edit proto files
vim protos/api.proto

# 2. Rebuild (regenerates code)
cargo build

# 3. Update code using the protos
# 4. Run tests
cargo test
```

## Linting and Formatting

### Clippy (Linter)

```bash
# Run clippy
cargo clippy

# Clippy with all features
cargo clippy --all-features

# Clippy for tests
cargo clippy --tests

# Treat warnings as errors
cargo clippy -- -D warnings
```

### Rustfmt (Formatter)

```bash
# Check formatting
cargo fmt --check

# Apply formatting
cargo fmt

# Format all workspace
cargo fmt --all
```

### Using Make

```bash
# Run all lints
make lint
```

## Common Tasks

### Add a New Dependency

```bash
# Add dependency
cargo add serde

# Add dev dependency
cargo add --dev proptest

# Add build dependency
cargo add --build prost-build

# Update dependencies
cargo update
```

### Add a New Module

```rust
// src/new_module.rs
pub fn new_function() -> String {
    "Hello".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_function() {
        assert_eq!(new_function(), "Hello");
    }
}
```

```rust
// src/lib.rs
mod new_module;
pub use new_module::new_function;
```

### Debug Build Issues

```bash
# Clean and rebuild
cargo clean
cargo build

# Check for outdated dependencies
cargo outdated

# Update Cargo.lock
cargo update
```

## Performance Optimization

### Profiling

```bash
# Build with debug symbols in release mode
cargo build --release --profile release-with-debug

# Use cargo-flamegraph
cargo install flamegraph
cargo flamegraph --bin your-bin
```

### Benchmarking

```bash
# Run benchmarks (if defined)
cargo bench

# Profile specific benchmark
cargo bench --bench resolver_bench
```

### Optimization Tips

- Use `--release` for production builds
- Profile before optimizing
- Consider using `#[inline]` for hot paths
- Use appropriate collection types
- Minimize allocations in hot paths

## Documentation

### Writing Documentation

```rust
/// Resolves a feature flag value.
///
/// # Arguments
///
/// * `flag_name` - The name of the flag to resolve
/// * `context` - The evaluation context
///
/// # Returns
///
/// The resolved flag value or an error
///
/// # Examples
///
/// ```
/// let value = resolve_flag("my-flag", context)?;
/// ```
pub fn resolve_flag(flag_name: &str, context: Context) -> Result<Value> {
    // Implementation
}
```

### Generating Documentation

```bash
# Build and open docs
cargo doc --open

# Build docs for all dependencies
cargo doc --open --document-private-items
```

## Troubleshooting

### Proto compilation fails

**Issue**: `protoc: command not found`

**Solution**:
```bash
brew install protobuf  # macOS
apt-get install protobuf-compiler  # Ubuntu
```

### Clippy warnings

**Issue**: Clippy suggests improvements

**Solution**: Address warnings or allow specific lints:
```rust
#[allow(clippy::lint_name)]
fn my_function() { }
```

### Build cache issues

**Solution**: Clean and rebuild:
```bash
cargo clean
cargo build
```

### Dependency conflicts

**Solution**: Update Cargo.lock:
```bash
cargo update
# Or update specific dependency
cargo update -p dependency-name
```

## Best Practices

### Code Organization

- Keep modules focused and cohesive
- Use `pub` sparingly - default to private
- Group related functionality
- Separate concerns (parsing, validation, resolution)

### Error Handling

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("Flag not found: {0}")]
    FlagNotFound(String),

    #[error("Invalid configuration")]
    InvalidConfig,
}

pub type Result<T> = std::result::Result<T, ResolverError>;
```

### API Design

- Use builder pattern for complex constructors
- Return `Result` for fallible operations
- Use strongly-typed wrappers over primitives
- Prefer iterators over collecting vectors

### Testing

- Test public API thoroughly
- Include integration tests in `tests/`
- Use doctests for examples
- Test error cases

### Performance

- Measure before optimizing
- Use `cargo bench` for benchmarks
- Avoid premature optimization
- Profile with real workloads

## Integration

The core resolver is used by:

1. **WASM guest** (`wasm/rust-guest`) - Compiles resolver to WASM
2. **Cloudflare Worker** (`confidence-cloudflare-resolver`) - Uses resolver in edge compute
3. **OpenFeature providers** - Indirectly via WASM

See respective CLAUDE.md files for integration details.

## Rust Edition

This crate uses **Rust 2021 edition**. Key features:

- Disjoint capture in closures
- IntoIterator for arrays
- Simplified cargo feature syntax
- Improved panic messages

## Additional Resources

- **[Repository root CLAUDE.md](../CLAUDE.md)** - Overall development guide
- **[wasm/rust-guest/CLAUDE.md](../wasm/rust-guest/CLAUDE.md)** - WASM compilation guide
- **Rust Book**: https://doc.rust-lang.org/book/
- **Rust by Example**: https://doc.rust-lang.org/rust-by-example/
- **Cargo Book**: https://doc.rust-lang.org/cargo/
- **Clippy Lints**: https://rust-lang.github.io/rust-clippy/
