//go:build race

package host

import (
	"runtime"
	"unsafe"
)

// The module synchronizes shared memory with WASM atomics. wazero's compiler emits those as native
// instructions, which the race detector can't see, so a buffer the module hands from one instance
// to another looks like a race between their Go-side copies. (The interpreter emulates atomics
// under a Go mutex, so it is unaffected.) Bracket every Go-side access to shared memory with
// acquire/release on one token per memory, to give the detector the ordering the module provides.

func raceAcquire(token *byte) { runtime.RaceAcquire(unsafe.Pointer(token)) }

func raceRelease(token *byte) { runtime.RaceReleaseMerge(unsafe.Pointer(token)) }
