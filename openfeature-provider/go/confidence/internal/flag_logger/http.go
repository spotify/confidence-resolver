package flag_logger

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"time"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/protobuf/proto"
)

const cloudflareIngestURL = "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest"

// httpSender sends flag logs via HTTP POST to the Cloudflare ingestor.
// It wraps the WriteFlagLogsRequest in an IngestFlagLogsRequest with the
// account ID before sending.
type httpSender struct {
	httpClient   *http.Client
	clientSecret string
	accountID    func() string
}

func newHttpSender(clientSecret string, accountID func() string, httpClient *http.Client) *httpSender {
	if httpClient == nil {
		httpClient = &http.Client{Timeout: 30 * time.Second}
	}
	return &httpSender{
		httpClient:   httpClient,
		clientSecret: clientSecret,
		accountID:    accountID,
	}
}

func (h *httpSender) send(ctx context.Context, request *resolverv1.WriteFlagLogsRequest) error {
	ingestReq := &resolverv1.IngestFlagLogsRequest{
		AccountId: h.accountID(),
		Batch:     request,
	}

	body, err := proto.Marshal(ingestReq)
	if err != nil {
		return fmt.Errorf("failed to marshal IngestFlagLogsRequest: %w", err)
	}

	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, cloudflareIngestURL, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("failed to create HTTP request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/protobuf")
	httpReq.Header.Set("Authorization", fmt.Sprintf("ClientSecret %s", h.clientSecret))

	resp, err := h.httpClient.Do(httpReq)
	if err != nil {
		return fmt.Errorf("HTTP request failed: %w", err)
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body) //nolint:errcheck

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return fmt.Errorf("unexpected HTTP status: %d", resp.StatusCode)
	}
	return nil
}
