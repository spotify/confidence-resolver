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

mkdir -p confidence/proto

# Generate wasm messages proto (WASM-specific types)
echo "Generating WASM messages..."
protoc --proto_path=../../wasm/proto \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mmessages.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       messages.proto

# Generate simplified resolver protos
echo "Generating resolver protos..."
mkdir -p confidence/proto/resolver
mkdir -p confidence/proto/resolverinternal
mkdir -p confidence/proto/admin

protoc --proto_path=../proto \
       --go_out=confidence/proto \
       --go_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       --go-grpc_out=confidence/proto \
       --go-grpc_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       confidence/resolver_types.proto \
       confidence/resolver_api.proto \
       confidence/resolver_wasm_api.proto \
       confidence/resolver_internal_api.proto \
       confidence/resolver_state.proto

echo "Protobuf generation complete!"
echo "Generated files:"
find confidence/proto -name "*.go" -type f
