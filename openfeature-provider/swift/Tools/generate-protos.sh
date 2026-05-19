#!/usr/bin/env bash
# Regenerate Swift protobuf bindings from openfeature-provider/proto.
#
# Requires:
#   - protoc            (brew install protobuf)
#   - protoc-gen-swift  (brew install swift-protobuf)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
PROTO_DIR="$(cd "${PKG_DIR}/../proto" && pwd)"
OUT_DIR="${PKG_DIR}/Sources/ConfidenceLocalResolver/Generated"

rm -f "${OUT_DIR}"/*.pb.swift

PROTOS=(
  "confidence/wasm/messages.proto"
  "confidence/wasm/wasm_api.proto"
  "confidence/flags/resolver/v1/api.proto"
  "confidence/flags/resolver/v1/types.proto"
  "confidence/flags/types/v1/types.proto"
  "confidence/flags/types/v1/target.proto"
)

protoc \
  --proto_path="${PROTO_DIR}" \
  --swift_out="${OUT_DIR}" \
  --swift_opt=Visibility=Public \
  --swift_opt=FileNaming=PathToUnderscores \
  "${PROTOS[@]/#/${PROTO_DIR}/}"

echo "Generated files:"
ls -1 "${OUT_DIR}"
