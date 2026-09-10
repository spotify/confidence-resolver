package confidence

// Logger is the logging interface accepted by [ProviderConfig] and [ProviderTestConfig].
//
// Implementations must be safe for concurrent use, as the provider calls Logger
// from multiple goroutines (state polling, log flushing, WASM crash recovery).
//
// The variadic args follow the slog convention: alternating string key / arbitrary
// value pairs, e.g.:
//
//	logger.Warn("retry limit reached", "attempt", 3, "flag", "my-flag")
//
// A nil Logger is accepted by [NewProvider] and [NewProviderForTest]; it is
// silently replaced with a no-op implementation.
type Logger interface {
	Debug(msg string, args ...any)
	Info(msg string, args ...any)
	Warn(msg string, args ...any)
	Error(msg string, args ...any)
}

// noopLogger is the default Logger used when none is provided by the caller.
type noopLogger struct{}

func (l *noopLogger) Debug(_ string, _ ...any) {}
func (l *noopLogger) Info(_ string, _ ...any)  {}
func (l *noopLogger) Warn(_ string, _ ...any)  {}
func (l *noopLogger) Error(_ string, _ ...any) {}
