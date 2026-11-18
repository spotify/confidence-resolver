# Root Makefile
# Local development commands - delegates to the providers' Makefiles.

# Providers rewritten on state-to-wasm modules. The others are API reference and don't build.
PROVIDERS := go

.PHONY: build test test-e2e lint clean all

# Each provider's Makefile has these targets (see CLAUDE.md).
build test test-e2e lint clean:
	@for p in $(PROVIDERS); do $(MAKE) -C openfeature-provider/$$p $@ || exit 1; done

# Everything that needs no credentials, like CI's default Docker target.
all: lint test build
	@echo "✅ All checks passed!"

.DEFAULT_GOAL := all
