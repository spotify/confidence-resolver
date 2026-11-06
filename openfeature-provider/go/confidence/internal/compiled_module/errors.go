package compiled_module

import (
	"errors"
	"fmt"
)

// Sentinel and constructor errors for the host package.
var (
	// errUnsupportedReservation marks platforms without a memory reservation
	// primitive; the allocator falls back to a bounded committed region.
	errUnsupportedReservation = errors.New("memory reservation unsupported on this platform")

	// errHostClosed is returned by instance acquisition after Close: the
	// host's runtime and shared memory are gone.
	errHostClosed = errors.New("module host is closed")

	errNilLogSink     = errors.New("compiled_module: LogSink is required")
	errMissingExport  = errors.New("compiled_module: instance is missing a required export")
	errUnexpectedImp  = "compiled_module: unexpected import"
	errEmptyEnvelope  = errors.New("module returned an empty resolve response")
	errSecondSuspend  = errors.New("module suspended a second time after resume; this is a module defect")
	errNilEnvelope    = errors.New("cannot evaluate without a resolve envelope")
	errEnvelopeClosed = errors.New("resolve envelope already freed")
)

func errUnexpectedImport(key string) error {
	return fmt.Errorf("%s %q", errUnexpectedImp, key)
}

// ResolveRejectedError is returned when the module answers a resolve with an
// error envelope. Code follows gRPC status values (e.g. 3 = INVALID_ARGUMENT,
// 16 = UNAUTHENTICATED).
type ResolveRejectedError struct {
	Code    int32
	Message string
}

func (e *ResolveRejectedError) Error() string {
	return fmt.Sprintf("resolve rejected: code %d: %s", e.Code, e.Message)
}

// IsUnauthenticated reports whether the module rejected the request as an
// unknown credential.
func (e *ResolveRejectedError) IsUnauthenticated() bool {
	// 16 = gRPC UNAUTHENTICATED
	return e.Code == 16
}

// ApplyStatus is the status code returned by the module's apply_flags export.
type ApplyStatus int32

const (
	ApplyOK            ApplyStatus = 0
	ApplyInvalid       ApplyStatus = 1
	ApplyUnknownFlag   ApplyStatus = 2
	ApplyDuplicate     ApplyStatus = 3
	ApplyUnknownClient ApplyStatus = 4
)

func (s ApplyStatus) String() string {
	switch s {
	case ApplyOK:
		return "ok"
	case ApplyInvalid:
		return "invalid request"
	case ApplyUnknownFlag:
		return "unknown flag"
	case ApplyDuplicate:
		return "duplicate flag"
	case ApplyUnknownClient:
		return "unknown client"
	default:
		return fmt.Sprintf("unknown status %d", int32(s))
	}
}
