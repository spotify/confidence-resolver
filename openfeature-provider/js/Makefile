# openfeature-provider/js Makefile
# TypeScript OpenFeature provider (yarn workspaces)

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and inputs
INSTALL_STAMP := .install.stamp
BUILD_STAMP := .build.stamp
SERVER_PKG := packages/server-provider
GEN_DIR := $(SERVER_PKG)/src/proto
GEN_TS := $(GEN_DIR)/test-only.ts $(GEN_DIR)/confidence/wasm/messages.ts $(GEN_DIR)/confidence/wasm/wasm_api.ts $(GEN_DIR)/confidence/flags/resolver/v1/types.ts $(GEN_DIR)/confidence/flags/resolver/v1/api.ts $(GEN_DIR)/confidence/flags/resolver/v1/internal_api.ts $(GEN_DIR)/confidence/flags/types/v1/types.ts
PROTO_SRC := proto/test-only.proto $(ROOT)/openfeature-provider/proto/confidence/wasm/messages.proto $(ROOT)/openfeature-provider/proto/confidence/wasm/wasm_api.proto $(ROOT)/openfeature-provider/proto/confidence/flags/resolver/v1/types.proto $(ROOT)/openfeature-provider/proto/confidence/flags/resolver/v1/api.proto $(ROOT)/openfeature-provider/proto/confidence/flags/resolver/v1/internal_api.proto $(ROOT)/openfeature-provider/proto/confidence/flags/types/v1/types.proto
SRC := $(shell find packages -name '*.ts' -not -path '*/node_modules/*' -not -path '*/dist/*')
CONFIG := package.json yarn.lock $(SERVER_PKG)/tsconfig.json $(SERVER_PKG)/tsdown.config.ts $(SERVER_PKG)/vitest.config.ts packages/react/tsconfig.json packages/react/tsdown.config.ts packages/react/vitest.config.ts
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
$(INSTALL_STAMP): package.json yarn.lock $(SERVER_PKG)/package.json packages/react/package.json
	yarn install --immutable
	@touch $@

# Generate TypeScript protos when sources or tools change
$(GEN_TS): $(PROTO_SRC) $(INSTALL_STAMP)
	@mkdir -p $(GEN_DIR)
	yarn workspace @spotify-confidence/openfeature-server-provider-local run proto:gen

$(BUILD_STAMP): $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS) $(SRC) $(CONFIG)
	yarn build
	@touch $@

install: $(INSTALL_STAMP) $(GEN_TS)

build: $(BUILD_STAMP)

test: $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS)
	yarn format:check
	yarn test --run --exclude='**/*.e2e.test.ts'

test-e2e: $(WASM_ARTIFACT) $(INSTALL_STAMP) $(GEN_TS)
	yarn workspace @spotify-confidence/openfeature-server-provider-local test --run e2e.test

bench-local: $(BUILD_STAMP)
	cd bench && DEBUG=* node bench.mjs -duration 30 -flag tutorial-feature -client-secret mkjJruAATQWjeY7foFIWfVAcBWnci2YF

clean:
	rm -rf node_modules $(SERVER_PKG)/node_modules packages/react/node_modules $(SERVER_PKG)/dist packages/react/dist $(GEN_DIR) $(INSTALL_STAMP) $(BUILD_STAMP)
