# confidence-event-engine Makefile

.PHONY: test lint clean

test:
	cargo test -p confidence-event-engine

lint:
	cargo clippy -p confidence-event-engine --lib --release -- -D warnings

clean:
	cargo clean
