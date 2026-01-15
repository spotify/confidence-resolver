# openfeature-provider/rust Makefile
# Rust OpenFeature provider (native resolver)

ROOT := $(realpath $(CURDIR)/../..)

# Stamps
BUILD_STAMP := .build.stamp

# Source files
RUST_SRC := $(shell find src -name '*.rs' 2>/dev/null)
CARGO_TOML := Cargo.toml

.PHONY: install build test test-e2e lint clean

# Install/update dependencies
install:
	cargo fetch

# Build the library
$(BUILD_STAMP): $(CARGO_TOML) $(RUST_SRC)
	cargo build
	@touch $@

build: $(BUILD_STAMP)

# Run unit tests
test: $(BUILD_STAMP)
	cargo test --lib

# Run e2e tests
test-e2e: $(BUILD_STAMP)
	cargo test --test e2e

# Lint using clippy and fmt check
lint:
	@echo "Checking format..."
	@cargo fmt --check
	@echo "Running clippy..."
	@cargo clippy -- -D warnings
	@echo "✅ Lint passed"

# Clean build artifacts
clean:
	cargo clean
	rm -f $(BUILD_STAMP)

.DEFAULT_GOAL := build
