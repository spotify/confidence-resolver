package flag_logger

import (
	"sync/atomic"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

// TelemetryCounters holds atomic counters for flush and event delivery
// telemetry. Embedded by both GrpcFlagLogger and MultiDestinationFlagLogger
// to avoid duplicating drain/restore/record logic.
type TelemetryCounters struct {
	FlushSucceeded        atomic.Int64
	FlushFailed           atomic.Int64
	EventsPublished       atomic.Int64
	EventBatchesSucceeded atomic.Int64
	EventBatchesFailed    atomic.Int64
	EventsRejected        atomic.Int64
}

// DrainAndStamp atomically drains all counters and stamps them onto the
// request's TelemetryData. Call before sending.
func (tc *TelemetryCounters) DrainAndStamp(request *resolverv1.WriteFlagLogsRequest) {
	succeeded := uint32(tc.FlushSucceeded.Swap(0))
	failed := uint32(tc.FlushFailed.Swap(0))
	evPub := uint32(tc.EventsPublished.Swap(0))
	evOk := uint32(tc.EventBatchesSucceeded.Swap(0))
	evFail := uint32(tc.EventBatchesFailed.Swap(0))
	evRejected := uint32(tc.EventsRejected.Swap(0))
	if succeeded > 0 || failed > 0 || evPub > 0 || evOk > 0 || evFail > 0 || evRejected > 0 {
		if request.TelemetryData == nil {
			request.TelemetryData = &resolverv1.TelemetryData{}
		}
		if succeeded > 0 || failed > 0 {
			request.TelemetryData.Flush = &resolverv1.TelemetryData_FlushTelemetry{
				Succeeded: succeeded,
				Failed:    failed,
			}
		}
		if evPub > 0 || evOk > 0 || evFail > 0 || evRejected > 0 {
			request.TelemetryData.Events = &resolverv1.TelemetryData_EventsTelemetry{
				Published:        evPub,
				BatchesSucceeded: evOk,
				BatchesFailed:    evFail,
				EventsRejected:   evRejected,
			}
		}
	}
}

// RestoreOnFailure adds the drained counters from the failed request back to
// the atomics so they are retried in the next flush. Also records the current
// failure (+1 to FlushFailed).
func (tc *TelemetryCounters) RestoreOnFailure(request *resolverv1.WriteFlagLogsRequest) {
	tc.FlushFailed.Add(1)
	if td := request.TelemetryData; td != nil {
		if td.Flush != nil {
			tc.FlushSucceeded.Add(int64(td.Flush.Succeeded))
			tc.FlushFailed.Add(int64(td.Flush.Failed))
		}
		if td.Events != nil {
			tc.EventsPublished.Add(int64(td.Events.Published))
			tc.EventBatchesSucceeded.Add(int64(td.Events.BatchesSucceeded))
			tc.EventBatchesFailed.Add(int64(td.Events.BatchesFailed))
			tc.EventsRejected.Add(int64(td.Events.EventsRejected))
		}
	}
}

// RecordEventBatch records an event batch delivery outcome. publishedCount is
// the number of events the service ingested (already net of rejections) and
// rejectedCount the number it refused within an otherwise successful batch.
func (tc *TelemetryCounters) RecordEventBatch(publishedCount, rejectedCount int, succeeded bool) {
	if succeeded {
		tc.EventsPublished.Add(int64(publishedCount))
		tc.EventBatchesSucceeded.Add(1)
		tc.EventsRejected.Add(int64(rejectedCount))
	} else {
		tc.EventBatchesFailed.Add(1)
	}
}
