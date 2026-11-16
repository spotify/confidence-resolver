#!/bin/bash
# Generates Go code for the state-to-wasm protos the provider uses into confidence/internal/s2wpb,
# from the shared copies in openfeature-provider/proto. Run from openfeature-provider/go.
set -euo pipefail
PROTOS=../proto
B=github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb
OUT=confidence/internal/s2wpb
FILES=(
  confidence/flags/wasm/v1/api.proto
  confidence/flags/wasm/v1/client_module.proto
  confidence/flags/wasm/v1/telemetry.proto
  confidence/flags/resolver/v1/api.proto
  confidence/flags/resolver/v1/types.proto
  confidence/flags/resolver/v1/events/events.proto
  confidence/flags/types/v1/types.proto
  confidence/flags/types/v1/target.proto
  confidence/flags/admin/v1/types.proto
)
M=()
for f in "${FILES[@]}"; do
  case $f in
    *wasm/v1*) p=wasmpb ;;
    *resolver/v1/events*) p=eventspb ;;
    *resolver/v1*) p=resolverpb ;;
    *types/v1*) p=typespb ;;
    *admin/v1*) p=adminpb ;;
  esac
  M+=("--go_opt=M$f=$B/$p")
done
rm -rf "$OUT" && mkdir -p "$OUT"
PATH=$PATH:$(go env GOPATH)/bin protoc -I "$PROTOS" "${M[@]}" \
  --go_opt=module="$B" --go_out="$OUT" "${FILES[@]}"
