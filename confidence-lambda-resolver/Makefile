.PHONY: build lint clean

build:
	cargo build --release --target aarch64-unknown-linux-gnu

lint:
	cargo clippy --release --target aarch64-unknown-linux-gnu -- -D warnings

clean:
	cargo clean
