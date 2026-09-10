package confidence_test

import (
	"bytes"
	"log/slog"
	"strings"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
)

func TestProviderConfigAcceptsSlogLogger(t *testing.T) {
	var output bytes.Buffer
	logger := slog.New(slog.NewTextHandler(&output, nil))
	config := confidence.ProviderConfig{Logger: logger}

	config.Logger.Info("hello from slog")

	if !strings.Contains(output.String(), "hello from slog") {
		t.Fatalf("expected slog output, got %q", output.String())
	}
}
