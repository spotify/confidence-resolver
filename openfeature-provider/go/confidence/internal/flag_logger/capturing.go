package flag_logger

import (
	"sync"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

// CapturingFlagLogger captures all WriteFlagLogsRequest objects for testing.
//
// This logger stores all requests in a thread-safe slice, allowing tests to verify:
//   - Flag names that were resolved
//   - Targeting keys used for evaluation
//   - Assignment IDs generated
//   - Variant information
//   - Client and credential information
//
// Usage example:
//
//	logger := NewCapturingFlagLogger()
//	// ... create provider with logger ...
//	// ... perform flag evaluations ...
//	// ... shutdown provider ...
//
//	requests := logger.GetCapturedRequests()
//	if len(requests) == 0 {
//	    t.Error("Expected captured requests")
//	}
type CapturingFlagLogger struct {
	mu               sync.Mutex
	capturedRequests []*resolverv1.WriteFlagLogsRequest
	shutdownCalled   bool
}

// NewCapturingFlagLogger creates a new CapturingFlagLogger
func NewCapturingFlagLogger() *CapturingFlagLogger {
	return &CapturingFlagLogger{
		capturedRequests: make([]*resolverv1.WriteFlagLogsRequest, 0),
	}
}

// Write captures the request for later inspection
func (c *CapturingFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.capturedRequests = append(c.capturedRequests, request)
}

// Shutdown marks that shutdown was called
func (c *CapturingFlagLogger) Shutdown() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.shutdownCalled = true
}

// GetCapturedRequests returns a copy of all captured requests
func (c *CapturingFlagLogger) GetCapturedRequests() []*resolverv1.WriteFlagLogsRequest {
	c.mu.Lock()
	defer c.mu.Unlock()
	result := make([]*resolverv1.WriteFlagLogsRequest, len(c.capturedRequests))
	copy(result, c.capturedRequests)
	return result
}

// Clear removes all captured requests
func (c *CapturingFlagLogger) Clear() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.capturedRequests = make([]*resolverv1.WriteFlagLogsRequest, 0)
}

// GetCapturedCount returns the number of captured requests
func (c *CapturingFlagLogger) GetCapturedCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.capturedRequests)
}

// WasShutdownCalled returns whether Shutdown was called
func (c *CapturingFlagLogger) WasShutdownCalled() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.shutdownCalled
}

// GetTotalFlagAssignedCount returns the total number of FlagAssigned entries
// across all captured requests
func (c *CapturingFlagLogger) GetTotalFlagAssignedCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	total := 0
	for _, req := range c.capturedRequests {
		total += len(req.FlagAssigned)
	}
	return total
}

// GetTotalClientResolveInfoCount returns the total number of ClientResolveInfo entries
// across all captured requests
func (c *CapturingFlagLogger) GetTotalClientResolveInfoCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	total := 0
	for _, req := range c.capturedRequests {
		total += len(req.ClientResolveInfo)
	}
	return total
}

// GetTotalFlagResolveInfoCount returns the total number of FlagResolveInfo entries
// across all captured requests
func (c *CapturingFlagLogger) GetTotalFlagResolveInfoCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	total := 0
	for _, req := range c.capturedRequests {
		total += len(req.FlagResolveInfo)
	}
	return total
}
