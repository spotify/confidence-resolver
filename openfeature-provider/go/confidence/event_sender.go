package confidence

import (
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// EventSender sends tracked events to the Confidence events backend.
type EventSender interface {
	Send(response *wasm.FlushEventsResponse, clientSecret string)
	Shutdown()
}
