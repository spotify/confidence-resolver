# Provider protos

Verbatim copies; don't edit them here.

- `confidence/` — from state-to-wasm `protos/src/main/proto` at
  `1a4005a1fd7ba92f86b13a32350a44942c8f2fd5`. Only the files providers use: the module API
  (`flags/wasm/v1`) and the messages it references.
- `google/api/field_behavior.proto`, `google/type/decimal.proto` — from
  `com.google.api.grpc:proto-google-common-protos:2.65.0`, the version state-to-wasm builds with.
  `google/protobuf/*` ships with protoc.

To update, copy the same files from a newer state-to-wasm commit, record its SHA above, and check
that the set still compiles:

```bash
protoc -I openfeature-provider/proto --descriptor_set_out=/dev/null \
  $(cd openfeature-provider/proto && find . -name '*.proto' | sed 's|^\./||')
```
