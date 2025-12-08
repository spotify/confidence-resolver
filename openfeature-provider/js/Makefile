# openfeature-provider/js Makefile
# TypeScript OpenFeature provider

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and inputs
INSTALL_STAMP := .install.stamp
BUILD_STAMP := .build.stamp
GEN_DIR := src/proto
GEN_TS := $(GEN_DIR)/messages.ts $(GEN_DIR)/resolver/api.ts $(GEN_DIR)/types.ts $(GEN_DIR)/confidence/flags/resolver/v1/types.ts
PROTO_SRC := proto/test-only.proto $(ROOT)/wasm/proto/messages.proto $(ROOT)/wasm/proto/resolver/api.proto $(ROOT)/wasm/proto/types.proto $(ROOT)/confidence-resolver/protos/confidence/flags/resolver/v1/types.proto
SRC := $(shell find src -name '*.ts')
CONFIG := package.json yarn.lock tsconfig.json tsdown.config.ts vitest.config.ts
WASM_ARTIFACT := $(ROOT)/wasm/confidence_resolver.wasm

.PHONY: install build test clean 

# Always build wasm if not in docker
ifneq ($(IN_DOCKER_BUILD),1)
.PHONY: build-wasm
build-wasm:
	@$(MAKE) -C $(ROOT) wasm/confidence_resolver.wasm
$(BUILD_STAMP): | build-wasm
endif

# Install dependencies only when package manifests change
$(INSTALL_STAMP): package.json yarn.lock
	yarn install --immutable
	@touch $@

# Generate TypeScript protos when sources or tools change
$(GEN_TS): $(PROTO_SRC) $(INSTALL_STAMP)
	@mkdir -p $(GEN_DIR)
	yarn proto:gen

$(BUILD_STAMP): $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS) $(SRC) $(CONFIG)
	yarn build
	@touch $@

install: $(INSTALL_STAMP) $(GEN_TS)

build: $(BUILD_STAMP)

test: $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS)
	yarn format:check
	yarn test --run --exclude='**/*.e2e.test.ts'

test-e2e: $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS)
	yarn test --run e2e.test

bench-local: $(BUILD_STAMP)
	cd bench && DEBUG=* node bench.mjs -duration 30 -flag tutorial-feature -client-secret mkjJruAATQWjeY7foFIWfVAcBWnci2YF

clean:
	rm -rf node_modules dist src/proto $(INSTALL_STAMP) $(BUILD_STAMP)

