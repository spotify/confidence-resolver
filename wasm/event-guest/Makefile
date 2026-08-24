# event-guest (WASM) Makefile
# Deployable WASM module. The guest also owns build_payload — the OpenFeature
# value/context -> Confidence payload mapping, including the key-collision
# rules — so its unit tests run natively via the `test` target.

.PHONY: build test lint clean

build:
	cargo build --target wasm32-unknown-unknown --profile wasm

# Runs on the host target: the tests exercise build_payload, not the WASM ABI.
test:
	cargo test -p event-guest

lint:
	cargo fmt --check
	cargo clippy --target wasm32-unknown-unknown --lib --release -- -D warnings

clean:
	cargo clean
