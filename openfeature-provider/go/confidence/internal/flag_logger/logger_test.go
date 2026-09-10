package flag_logger

import (
	"bytes"
	"fmt"
	"strings"
	"testing"
)

// testingLogger accepts log messages without retaining test state.
type testingLogger struct {
	log func(msgs ...any)
}

// newLoggerForTest returns a no-op [Logger] safe for background goroutines.
func newLoggerForTest(_ testing.TB) *testingLogger {
	return &testingLogger{
		log: func(...any) {},
	}
}

func (l *testingLogger) Debug(msg string, args ...any) {
	l.logMessage("debug", msg, args...)
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

// warningRecorderLogger captures only Warn calls so tests can assert on warning
// output without noise from Debug messages.
type warningRecorderLogger struct {
	buf bytes.Buffer
}

// newWarningRecorderLogger returns an empty [warningRecorderLogger].
func newWarningRecorderLogger() *warningRecorderLogger {
	return &warningRecorderLogger{}
}

func (l *warningRecorderLogger) Debug(msg string, args ...any) {}

func (l *warningRecorderLogger) Warn(msg string, args ...any) {
	l.buf.WriteString(msg)
	defer l.buf.WriteRune('\n')

	for len(args) > 0 {
		k, v := args[0].(string), args[1]
		l.buf.WriteString(fmt.Sprintf(" %s=%v", k, v))
		args = args[2:]
	}
}

func (l *warningRecorderLogger) String() string {
	return l.buf.String()
}
