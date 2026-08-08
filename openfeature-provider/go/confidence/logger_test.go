package confidence

import (
	"bytes"
	"fmt"
	"strings"
	"sync"
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

func (l *testingLogger) Debug(msg string, args ...any) {
	l.logMessage("debug", msg, args...)
}

func (l *testingLogger) Info(msg string, args ...any) {
	l.logMessage("info", msg, args...)
}

func (l *testingLogger) Warn(msg string, args ...any) {
	l.logMessage("warning", msg, args...)
}

func (l *testingLogger) Error(msg string, args ...any) {
	l.logMessage("error", msg, args...)
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

// recorderLogger accumulates all log lines in a buffer so tests can assert on
// logged output. It is safe for concurrent use.
type recorderLogger struct {
	l   sync.Mutex
	buf bytes.Buffer
}

// newRecorderLogger returns an empty [recorderLogger].
func newRecorderLogger() *recorderLogger {
	return &recorderLogger{}
}

func (l *recorderLogger) Debug(msg string, args ...any) {
	l.log(msg, args...)
}

func (l *recorderLogger) Info(msg string, args ...any) {
	l.log(msg, args...)
}

func (l *recorderLogger) Warn(msg string, args ...any) {
	l.log(msg, args...)
}

func (l *recorderLogger) Error(msg string, args ...any) {
	l.log(msg, args...)
}

func (l *recorderLogger) log(msg string, args ...any) {
	l.l.Lock()
	defer l.l.Unlock()

	l.buf.WriteString(msg)
	defer l.buf.WriteRune('\n')

	for len(args) > 0 {
		k, v := args[0].(string), args[1]
		l.buf.WriteString(fmt.Sprintf(" %s=%v", k, v))
		args = args[2:]
	}
}

func (l *recorderLogger) String() string {
	l.l.Lock()
	defer l.l.Unlock()

	return l.buf.String()
}
