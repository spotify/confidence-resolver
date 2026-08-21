# event-guest (WASM) Makefile
# Deployable WASM module - no tests (logic tested in confidence-event-engine)

.PHONY: build lint clean

build:
	cargo build --target wasm32-unknown-unknown --profile wasm

lint:
	cargo fmt --check
	cargo clippy --target wasm32-unknown-unknown --lib --release -- -D warnings

clean:
	cargo clean
