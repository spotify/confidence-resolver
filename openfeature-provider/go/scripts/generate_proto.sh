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

# Generate protobuf Go files
echo "Generating protobuf Go files..."

mkdir -p confidence/proto

# Generate wasm messages proto (WASM-specific types)
protoc --proto_path=../../wasm/proto \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mmessages.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       messages.proto

# Generate base types and annotations first (these have no confidence dependencies)
mkdir -p confidence/proto/confidence/auth/v1
mkdir -p confidence/proto/confidence/api
mkdir -p confidence/proto/confidence/events/v1
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       confidence/auth/v1/auth.proto \
       confidence/api/annotations.proto \
       confidence/events/v1/annotations.proto

# Generate flag types (no confidence dependencies except annotations)
mkdir -p confidence/proto/confidence/flags/types/v1
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       confidence/flags/types/v1/types.proto \
       confidence/flags/types/v1/target.proto

# Generate IAM proto (depends on auth)
mkdir -p confidence/proto/confidence/iam/v1
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/iam/v1/iam.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       confidence/iam/v1/iam.proto

# Generate admin events and types (depend on flags/types)
mkdir -p confidence/proto/confidence/flags/admin/v1/events
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/flags/admin/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1/events \
       --go_opt=Mconfidence/flags/admin/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       confidence/flags/admin/v1/events/events.proto \
       confidence/flags/admin/v1/types.proto

# Generate admin resolver state service (depends on admin types, flags types, iam, auth)
mkdir -p confidence/proto/confidence/flags/admin/v1
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/flags/admin/v1/resolver.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go_opt=Mconfidence/flags/admin/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/iam/v1/iam.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go_opt=Mconfidence/flags/admin/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1/events \
       --go-grpc_out=confidence/proto \
       --go-grpc_opt=paths=source_relative \
       --go-grpc_opt=Mconfidence/flags/admin/v1/resolver.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go-grpc_opt=Mconfidence/flags/admin/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go-grpc_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go-grpc_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go-grpc_opt=Mconfidence/iam/v1/iam.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go-grpc_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go-grpc_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go-grpc_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go-grpc_opt=Mconfidence/flags/admin/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1/events \
       confidence/flags/admin/v1/resolver.proto

# Generate internal flag logger service
mkdir -p confidence/proto/confidence/flags/resolverinternal
mkdir -p confidence/proto/confidence/flags/resolvertypes
mkdir -p confidence/proto/confidence/flags/resolverevents
protoc --proto_path=../proto \
       --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       --go_opt=Mconfidence/flags/resolver/v1/internal_api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolverinternal \
       --go_opt=Mconfidence/flags/resolver/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolvertypes \
       --go_opt=Mconfidence/flags/resolver/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolverevents \
       --go_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go_opt=Mconfidence/flags/admin/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go_opt=Mconfidence/flags/admin/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1/events \
       --go_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go-grpc_out=confidence/proto \
       --go-grpc_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       --go-grpc_opt=Mconfidence/flags/resolver/v1/internal_api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolverinternal \
       --go-grpc_opt=Mconfidence/flags/resolver/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolvertypes \
       --go-grpc_opt=Mconfidence/flags/resolver/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolverevents \
       --go-grpc_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go-grpc_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go-grpc_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go-grpc_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go-grpc_opt=Mconfidence/flags/admin/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1 \
       --go-grpc_opt=Mconfidence/flags/admin/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/admin/v1/events \
       --go-grpc_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       confidence/flags/resolver/v1/internal_api.proto \
       confidence/flags/resolver/v1/types.proto \
       confidence/flags/resolver/v1/events/events.proto

# Generate auth service with gRPC stubs
mkdir -p confidence/proto/confidence/iam/v1
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=paths=source_relative \
       --go_opt=Mconfidence/iam/v1/auth_api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go_opt=Mconfidence/iam/v1/iam.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go-grpc_out=confidence/proto \
       --go-grpc_opt=paths=source_relative \
       --go-grpc_opt=Mconfidence/iam/v1/auth_api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go-grpc_opt=Mconfidence/iam/v1/iam.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/iam/v1 \
       --go-grpc_opt=Mconfidence/auth/v1/auth.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/auth/v1 \
       --go-grpc_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go-grpc_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       confidence/iam/v1/auth_api.proto

# Note: All supporting types have been generated in previous steps

# Generate resolver API and WASM API proto (for resolver interface) - must be last as it depends on other protos
mkdir -p proto/resolver
protoc --proto_path=../../confidence-resolver/protos \
       --proto_path=../java/target/protoc-dependencies/fb94b2d0c5936e4cf7aa794a2caf00da \
       --proto_path=../java/target/protoc-dependencies/45da6e25a3df602921e82a52a83b342b \
       --go_out=confidence/proto \
       --go_opt=module=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto \
       --go_opt=Mconfidence/flags/resolver/v1/api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolver \
       --go_opt=Mconfidence/flags/resolver/v1/wasm_api.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolver \
       --go_opt=Mconfidence/flags/resolver/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolvertypes \
       --go_opt=Mconfidence/flags/types/v1/types.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/flags/types/v1/target.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/types/v1 \
       --go_opt=Mconfidence/api/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/api \
       --go_opt=Mconfidence/events/v1/annotations.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/events/v1 \
       --go_opt=Mconfidence/flags/resolver/v1/events/events.proto=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/confidence/flags/resolverevents \
       confidence/flags/resolver/v1/api.proto \
       confidence/flags/resolver/v1/wasm_api.proto

echo "Protobuf generation complete!"
echo "Generated files:"
find confidence/proto -name "*.go" -type f
