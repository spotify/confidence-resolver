#!/bin/bash

set -e

# Add Go bin to PATH
export PATH=$PATH:$(go env GOPATH)/bin

# Check for required tools
if ! command -v protoc &> /dev/null; then
    echo "Error: protoc is not installed. Please install Protocol Buffers compiler." >&2
    exit 1
fi

if ! command -v protoc-gen-go &> /dev/null; then
    echo "Error: protoc-gen-go is not installed." >&2
    echo "Install with: go install google.golang.org/protobuf/cmd/protoc-gen-go@latest" >&2
    exit 1
fi

if ! command -v protoc-gen-go-grpc &> /dev/null; then
    echo "Error: protoc-gen-go-grpc is not installed." >&2
    echo "Install with: go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest" >&2
    exit 1
fi

echo "Generating protobuf Go files..."

mkdir -p confidence/internal/proto

# Generate wasm messages proto (WASM-specific types)
# Generate simplified resolver protos
echo "Generating WASM messages & resolver protos..."
mkdir -p confidence/internal/proto/resolver
mkdir -p confidence/internal/proto/resolverinternal
mkdir -p confidence/internal/proto/admin
mkdir -p confidence/internal/proto/types
mkdir -p confidence/internal/proto/wasm

protoc --proto_path=../proto \
       --go_out=confidence/internal/proto \
       --go_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto \
       --go-grpc_out=confidence/internal/proto \
       --go-grpc_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto \
       confidence/flags/types/v1/types.proto \
       confidence/flags/types/v1/target.proto \
       confidence/flags/resolver/v1/types.proto \
       confidence/flags/resolver/v1/api.proto \
       confidence/flags/resolver/v1/internal_api.proto \
       confidence/flags/admin/v1/resolver.proto \
       confidence/wasm/wasm_api.proto \
       confidence/wasm/messages.proto

echo "Protobuf generation complete!"
echo "Generated files:"
find confidence/internal/proto -name "*.go" -type f
