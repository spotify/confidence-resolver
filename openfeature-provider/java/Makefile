# openfeature-provider/java Makefile
# Java OpenFeature local provider

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and inputs
BUILD_STAMP := .build.stamp
SRC := $(shell find src -name '*.java' 2>/dev/null)
RESOURCES_WASM := src/main/resources/wasm/confidence_resolver.wasm
LOCAL_WASM := $(ROOT)/wasm/confidence_resolver.wasm

.PHONY: build test test-e2e clean

# Always build wasm if not in docker
ifneq ($(IN_DOCKER_BUILD),1)
.PHONY: build-wasm
build-wasm:
	@$(MAKE) -C $(ROOT) wasm/confidence_resolver.wasm
$(BUILD_STAMP): | build-wasm
endif

$(RESOURCES_WASM): $(LOCAL_WASM)
	@mkdir -p $(dir $@)
	@cp -p $(LOCAL_WASM) $@

$(BUILD_STAMP): pom.xml $(RESOURCES_WASM) $(SRC)
	mvn -q package -DskipTests
	@touch $@

build: $(BUILD_STAMP)

test: $(BUILD_STAMP)
	mvn -q test -Dtest='!**/*E2ETest'

test-e2e: $(BUILD_STAMP)
	mvn -q verify -Dsurefire.skip=true
	
clean:
	mvn -q clean
	rm -f $(BUILD_STAMP)
	rm -f $(RESOURCES_WASM)


