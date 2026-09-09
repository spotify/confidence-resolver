# Root Makefile
# Local development commands - delegates to component Makefiles

TARGET_WASM := target/wasm32-unknown-unknown/wasm/rust_guest.wasm
TARGET_EVENT_WASM := target/wasm32-unknown-unknown/wasm/event_guest.wasm
GO_WASM := openfeature-provider/go/confidence/internal/local_resolver/assets
GO_EVENT_WASM := openfeature-provider/go/confidence/internal/event_tracking/assets

.PHONY: $(TARGET_WASM) $(TARGET_EVENT_WASM) test lint build all clean

$(TARGET_WASM):
	@$(MAKE) -C wasm/rust-guest build

$(TARGET_EVENT_WASM):
	@$(MAKE) -C wasm/event-guest build

wasm/confidence_resolver.wasm: $(TARGET_WASM)
	@mkdir -p wasm
	@cp -p $(TARGET_WASM) $@
	@echo "WASM size: $$(ls -lh $@ | awk '{print $$5}')"

wasm/confidence_event_engine.wasm: $(TARGET_EVENT_WASM)
	@mkdir -p wasm
	@cp -p $(TARGET_EVENT_WASM) $@
	@echo "Event WASM size: $$(ls -lh $@ | awk '{print $$5}')"

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

# Sync event engine WASM to Go provider using Docker to ensure correct toolchain
.PHONY: sync-wasm-event-go
sync-wasm-event-go:
	@echo "Building event engine WASM with Docker to ensure correct dependencies..."
	@docker build --platform linux/arm64 --target wasm-event-guest.artifact --output type=local,dest=$(GO_EVENT_WASM) .
	@echo "✅ Event WASM synced to $(GO_EVENT_WASM)/"
	@echo ""
	@echo "Don't forget to commit the change:"
	@echo "  git add $(GO_EVENT_WASM)/confidence_event_engine.wasm"
	@echo "  git commit -m 'chore: sync event engine WASM for Go provider'"

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
	$(MAKE) -C confidence-event-engine test
	$(MAKE) -C wasm/event-guest test
	$(MAKE) -C wasm-msg test
	$(MAKE) -C openfeature-provider/js test
	$(MAKE) -C openfeature-provider/java test
	$(MAKE) -C openfeature-provider/go test
	$(MAKE) -C openfeature-provider/ruby test
	$(MAKE) -C openfeature-provider/rust test
	$(MAKE) -C openfeature-provider/python test

lint:
	$(MAKE) -C confidence-resolver lint
	$(MAKE) -C confidence-event-engine lint
	$(MAKE) -C wasm-msg lint
	$(MAKE) -C wasm/rust-guest lint
	$(MAKE) -C wasm/event-guest lint
	$(MAKE) -C confidence-cloudflare-resolver lint
	$(MAKE) -C openfeature-provider/go lint
	$(MAKE) -C openfeature-provider/ruby lint
	$(MAKE) -C openfeature-provider/rust lint
	cargo fmt --check -p wasm-msg -p rust-guest -p event-guest -p confidence_resolver -p confidence-event-engine -p confidence-cloudflare-resolver -p spotify-confidence-openfeature-provider

build: wasm/confidence_resolver.wasm wasm/confidence_event_engine.wasm
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

.PHONY: go-bench js-bench wasm-state-scaling wasm-state-growth
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

# Opt-in WASM linear-memory scaling harness. Override these on the make command
# line for boundary runs, for example: make wasm-state-scaling WASM_STATE_SCALING_MIB=64,128,256
WASM_STATE_SCALING_MIB ?= 1,4,16,64
WASM_STATE_SCALING_MODES ?= random,sparse,zero
WASM_STATE_SCALING_RESOLVES ?= 1000
wasm-state-scaling:
	cd openfeature-provider/go && \
	CONFIDENCE_WASM_STATE_SCALING=1 \
	CONFIDENCE_WASM_STATE_SCALING_MIB=$(WASM_STATE_SCALING_MIB) \
	CONFIDENCE_WASM_STATE_SCALING_MODES=$(WASM_STATE_SCALING_MODES) \
	CONFIDENCE_WASM_STATE_SCALING_RESOLVES=$(WASM_STATE_SCALING_RESOLVES) \
	go test ./confidence/internal/local_resolver -run TestWasmStateScaling -v -count=1

# Continuously append bitsets and replace the state in one WASM instance.
# The default 512 MiB guest cap makes the failure observable without attempting
# to consume the full memory32 address space. Set the limit to 0 for no test cap.
WASM_STATE_GROWTH_STEP_MIB ?= 8
WASM_STATE_GROWTH_MAX_MIB ?= 256
WASM_STATE_GROWTH_MODE ?= random
WASM_STATE_GROWTH_MEMORY_LIMIT_MIB ?= 512
wasm-state-growth:
	cd openfeature-provider/go && \
	CONFIDENCE_WASM_STATE_GROWTH=1 \
	CONFIDENCE_WASM_STATE_GROWTH_STEP_MIB=$(WASM_STATE_GROWTH_STEP_MIB) \
	CONFIDENCE_WASM_STATE_GROWTH_MAX_MIB=$(WASM_STATE_GROWTH_MAX_MIB) \
	CONFIDENCE_WASM_STATE_GROWTH_MODE=$(WASM_STATE_GROWTH_MODE) \
	CONFIDENCE_WASM_STATE_GROWTH_MEMORY_LIMIT_MIB=$(WASM_STATE_GROWTH_MEMORY_LIMIT_MIB) \
	CONFIDENCE_WASM_STATE_SCALING_RESOLVES=$(WASM_STATE_SCALING_RESOLVES) \
	go test ./confidence/internal/local_resolver -run TestWasmStateContinuousGrowth -v -count=1

.DEFAULT_GOAL := all
