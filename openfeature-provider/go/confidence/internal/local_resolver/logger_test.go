package local_resolver

import (
	"fmt"
	"strings"
	"testing"
)

// testingLogger routes log messages to [testing.TB.Log] so they appear in test
// output only on failure (or with -v).
type testingLogger struct {
	log func(msgs ...any)
}

// newLoggerForTest returns a [Logger] backed by tb.Log.
func newLoggerForTest(tb testing.TB) *testingLogger {
	return &testingLogger{
		log: tb.Log,
	}
}

func (l *testingLogger) Warn(msg string, args ...any) {
	l.logMessage("warning", msg, args...)
}

func (l *testingLogger) logMessage(lvl string, msg string, args ...any) {
	var fields string

	if len(args) > 0 {
		var sb strings.Builder

		sb.WriteRune('{')
		for len(args) > 0 {
			k, v := args[0].(string), args[1]
			if sb.Len() > 1 {
				sb.WriteString(", ")
			}
			sb.WriteString(fmt.Sprintf("%s: %v", k, v))
			args = args[2:]
		}
		sb.WriteRune('}')

		fields = sb.String()
	}

	l.log(fmt.Sprintf("[%s] %s", lvl, msg), fields)
}
