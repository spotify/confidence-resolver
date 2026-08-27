package event_tracking

import _ "embed"

// EventEngineWasm is the compiled event engine WASM module, embedded at build
// time so callers of the public provider API don't have to supply it.
// Kept in sync with wasm/event-guest via `make sync-wasm-event-go`; CI enforces
// that the committed bytes match a fresh Docker build.
//
//go:embed assets/confidence_event_engine.wasm
var EventEngineWasm []byte
