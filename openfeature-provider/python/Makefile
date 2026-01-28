# openfeature-provider/python Makefile
# Python OpenFeature local provider

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and inputs
INSTALL_STAMP := .install.stamp
BUILD_STAMP := .build.stamp
VENV := .venv
PYTHON := $(VENV)/bin/python
PIP := $(VENV)/bin/pip
SRC := $(shell find src -name '*.py' 2>/dev/null)
TESTS := $(shell find tests -name '*.py' 2>/dev/null)
RESOURCES_WASM := resources/wasm/confidence_resolver.wasm
# WASM is built from source in wasm/rust-guest, output goes to wasm/confidence_resolver.wasm
WASM_SOURCE := $(ROOT)/wasm/confidence_resolver.wasm

.PHONY: install build test test-e2e lint proto clean wasm format venv

# In Docker, WASM is copied directly from the wasm-rust-guest.artifact stage
# Locally, we build WASM from source to ensure it's always in sync
ifeq ($(IN_DOCKER_BUILD),1)
# In Docker, WASM already exists - don't try to build/copy
$(RESOURCES_WASM):
	@test -f $@ || (echo "Error: WASM file not found at $@" && exit 1)
else
# Locally, build WASM from source (like Go provider does)
$(WASM_SOURCE):
	@echo "Building WASM from source..."
	@$(MAKE) -C $(ROOT) wasm/confidence_resolver.wasm

# Copy freshly built WASM to resources
$(RESOURCES_WASM): $(WASM_SOURCE)
	@mkdir -p $(dir $@)
	@cp -p $(WASM_SOURCE) $@
endif

# Create virtual environment
$(VENV)/bin/activate:
	python3 -m venv $(VENV)
	$(PIP) install --upgrade pip

venv: $(VENV)/bin/activate

# Install dependencies only when pyproject.toml changes
$(INSTALL_STAMP): $(VENV)/bin/activate pyproject.toml
	$(PIP) install -e ".[dev]"
	@touch $@

install: $(INSTALL_STAMP)

# Build package
$(BUILD_STAMP): pyproject.toml $(INSTALL_STAMP) $(RESOURCES_WASM) $(SRC)
	$(PIP) install build
	$(PYTHON) -m build
	@touch $@

build: $(BUILD_STAMP)

# Run unit tests
test: $(INSTALL_STAMP) $(RESOURCES_WASM)
	$(PYTHON) -m pytest tests/ -v --ignore=tests/e2e_test.py --ignore=tests/flaglogs_e2e_test.py

# Run e2e tests
test-e2e: $(INSTALL_STAMP) $(RESOURCES_WASM)
	$(PYTHON) -m pytest tests/e2e_test.py tests/flaglogs_e2e_test.py -v

# Run linting
lint: $(INSTALL_STAMP)
	$(PYTHON) -m black --check src/ tests/
	$(PYTHON) -m flake8 src/ tests/
	$(PYTHON) -m mypy src/

# Format code
format: $(INSTALL_STAMP)
	$(PYTHON) -m black src/ tests/

# Generate protobuf files
proto: $(INSTALL_STAMP)
	$(PYTHON) -m grpc_tools.protoc \
		-I../proto \
		--python_out=src/confidence/proto \
		--grpc_python_out=src/confidence/proto \
		../proto/confidence/flags/resolver/v1/*.proto \
		../proto/confidence/wasm/*.proto

# Copy WASM file (alias for compatibility)
wasm: $(RESOURCES_WASM)

# Clean build artifacts
clean:
	rm -rf build/ dist/ *.egg-info .pytest_cache .mypy_cache
	rm -f $(INSTALL_STAMP) $(BUILD_STAMP)
	rm -f $(RESOURCES_WASM)
	find . -type d -name __pycache__ -exec rm -rf {} +

.DEFAULT_GOAL := build
