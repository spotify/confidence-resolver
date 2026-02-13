# Root Makefile
# Local development commands - delegates to component Makefiles

TARGET_WASM := target/wasm32-unknown-unknown/wasm/rust_guest.wasm
GO_WASM := openfeature-provider/go/confidence/internal/local_resolver/assets

.PHONY: $(TARGET_WASM) test lint build all clean

$(TARGET_WASM):
	@$(MAKE) -C wasm/rust-guest build

wasm/confidence_resolver.wasm: $(TARGET_WASM)
	@mkdir -p wasm
	@cp -p $(TARGET_WASM) $@
	@echo "WASM size: $$(ls -lh $@ | awk '{print $$5}')"

# Sync WASM to Go provider using Docker to ensure correct toolchain
.PHONY: sync-wasm-go
sync-wasm-go:
	@echo "Building WASM with Docker to ensure correct dependencies..."
	@docker build --platform linux/arm64 --target wasm-rust-guest.artifact --output type=local,dest=$(GO_WASM) .
	@echo "✅ WASM synced to $(GO_WASM)/"
	@echo ""
	@echo "Don't forget to commit the change:"
	@echo "  git add $(GO_WASM)/confidence_resolver.wasm"
	@echo "  git commit -m 'chore: sync WASM module for Go provider'"

# Build Cloudflare deployer image using main Dockerfile
.PHONY: build-deployer
build-deployer:
	@echo "Building Cloudflare deployer image with shared cache..."
	@docker build \
		--target confidence-cloudflare-resolver.deployer \
		--build-arg COMMIT_SHA=$$(git rev-parse HEAD) \
		-t confidence-cloudflare-deployer:latest \
		.
	@echo "✅ Deployer image built: confidence-cloudflare-deployer:latest"

test:
	$(MAKE) -C confidence-resolver test
	$(MAKE) -C wasm-msg test
	$(MAKE) -C openfeature-provider/js test
	$(MAKE) -C openfeature-provider/java test
	$(MAKE) -C openfeature-provider/go test
	$(MAKE) -C openfeature-provider/ruby test
	$(MAKE) -C openfeature-provider/rust test
	$(MAKE) -C openfeature-provider/python test

lint:
	$(MAKE) -C confidence-resolver lint
	$(MAKE) -C wasm-msg lint
	$(MAKE) -C wasm/rust-guest lint
	$(MAKE) -C confidence-cloudflare-resolver lint
	$(MAKE) -C openfeature-provider/go lint
	$(MAKE) -C openfeature-provider/ruby lint
	$(MAKE) -C openfeature-provider/rust lint
	cargo fmt --check -p wasm-msg -p rust-guest -p confidence_resolver -p confidence-cloudflare-resolver -p spotify-confidence-openfeature-provider

build: wasm/confidence_resolver.wasm
	$(MAKE) -C openfeature-provider/js build
	$(MAKE) -C openfeature-provider/java build
	$(MAKE) -C openfeature-provider/go build
	$(MAKE) -C openfeature-provider/ruby build
	$(MAKE) -C openfeature-provider/rust build
	$(MAKE) -C openfeature-provider/python wasm

all: lint test build
	@echo "✅ All checks passed!"

clean:
	cargo clean
	$(MAKE) -C openfeature-provider/js clean
	$(MAKE) -C openfeature-provider/java clean
	$(MAKE) -C openfeature-provider/go clean
	$(MAKE) -C openfeature-provider/ruby clean
	$(MAKE) -C openfeature-provider/rust clean
	$(MAKE) -C openfeature-provider/python clean

.PHONY: js-build
js-build:
	$(MAKE) -C openfeature-provider/js build

.PHONY: go-bench js-bench
go-bench:
	@status=0; \
	docker compose up --build \
		--abort-on-container-exit \
		--exit-code-from go-bench \
		--attach go-bench --attach mock-support \
		go-bench mock-support || status=$$?; \
	docker compose down --remove-orphans --volumes; \
	exit $$status

js-bench: js-build
	@status=0; \
	docker compose up --build \
		--abort-on-container-exit \
		--exit-code-from js-bench \
		--attach js-bench --attach mock-support \
		js-bench mock-support || status=$$?; \
	docker compose down --remove-orphans --volumes; \
	exit $$status

.DEFAULT_GOAL := all
