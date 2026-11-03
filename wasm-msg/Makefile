# wasm-msg Makefile
# Support library

.PHONY: test lint clean

test:
	cargo test --release --lib

lint:
	cargo fmt --check
	cargo clippy --lib --release -- -D warnings

clean:
	cargo clean

