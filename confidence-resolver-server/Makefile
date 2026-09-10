.PHONY: build test lint image clean

build:
	cargo build --locked --release -p confidence-resolver-server

test:
	cargo test --locked -p confidence-resolver-server

lint:
	cargo fmt --check -p confidence-resolver-server
	cargo clippy --locked -p confidence-resolver-server --no-deps --all-targets -- -D warnings

image:
	docker build --target confidence-resolver-server.runtime -t confidence-resolver-server:local ..

clean:
	cargo clean -p confidence-resolver-server
