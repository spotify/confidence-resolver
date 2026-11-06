# openfeature-provider/go Makefile
# Go OpenFeature local provider hosting compiled client modules

ROOT := $(realpath $(CURDIR)/../..)

# Stamps and directories
BUILD_STAMP := .build.stamp

# Source files
GO_SRC := $(shell find confidence -name '*.go')

.PHONY: install build test lint clean proto

# Install Go dependencies
install:
	go mod download
	go mod verify

# Regenerate protobuf files
proto:
	./scripts/generate_proto.sh

# Build (compile check)
$(BUILD_STAMP): go.mod go.sum $(GO_SRC)
	go build ./...
	@touch $@

build: $(BUILD_STAMP)

# Run tests
test: $(BUILD_STAMP)
	go test -v ./...

# Lint using gofmt and go vet
lint:
	@echo "Checking gofmt..."
	@test -z "$$(gofmt -l . | grep -v '^proto/' | tee /dev/stderr)" || (echo "Files need formatting. Run: gofmt -w ." && exit 1)
	@echo "Running go vet..."
	@go vet ./...
	@echo "✅ Lint passed"

# Clean build artifacts
clean:
	rm -f $(BUILD_STAMP)
	cd confidence && go clean -cache -testcache -modcache

.DEFAULT_GOAL := build
