# Root Makefile
# Local development commands - delegates to the providers' Makefiles.

# Providers rewritten on state-to-wasm modules. The others are API reference and don't build.
PROVIDERS :=

.PHONY: test lint build all clean

test lint build clean:
	@for p in $(PROVIDERS); do $(MAKE) -C openfeature-provider/$$p $@ || exit 1; done

all: lint test build
	@echo "✅ All checks passed!"

.DEFAULT_GOAL := all
