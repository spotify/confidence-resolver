package local_resolver

// Logger is the subset of the top-level confidence.Logger interface required by
// this package.
//
// Only Warn is used: to report WASM instance crashes caught by [RecoveringResolver]
// and to surface pool saturation events.
//
// The variadic args follow the slog convention: alternating string key / arbitrary
// value pairs.
type Logger interface {
	Warn(msg string, args ...any)
}

// noopLogger is the default Logger used when none is provided by the caller.
type noopLogger struct{}

func (l *noopLogger) Warn(_ string, _ ...any) {}
