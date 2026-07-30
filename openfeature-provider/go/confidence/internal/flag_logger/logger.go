package flag_logger

// Logger is the subset of the top-level confidence.Logger interface required by
// this package.
//
// Only Debug and Warn are used: Debug for successful send traces and Warn for
// periodic failure summaries reported every 10 write attempts.
//
// The variadic args follow the slog convention: alternating string key / arbitrary
// value pairs.
type Logger interface {
	Debug(msg string, args ...any)
	Warn(msg string, args ...any)
}
