package confidence

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/events"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/eventswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/grpc"
	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	cm "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/compiled_module"
)

const (
	defaultModulePollIntervalSeconds = 30
	defaultLogFlushInterval          = 5 * time.Second
	defaultInitializationTimeout     = 30 * time.Second
	initialModuleRetryInterval       = time.Second
)

// LocalResolverProvider implements the OpenFeature FeatureProvider interface
// by hosting the credential's compiled client module: a WASM module that
// already contains the credential's flag rules, fetched as an encrypted
// artifact from the module CDN and executed locally. There is no state to
// load into a resolver at runtime; loading a module is loading its flags.
type LocalResolverProvider struct {
	clientSecret string
	logger       *slog.Logger

	fetcher   moduleFetcher
	forwarder logForwarder

	// hostMu guards the live host swap. Evaluation paths hold the read side
	// for the whole operation (resolve through evaluate), so a replaced host
	// is only destroyed after every in-flight call has drained.
	hostMu sync.RWMutex
	host   *cm.Host
	// moduleBytes is the module the live host was built from; a fetch that
	// returns the same bytes with a new log token is a wrapper-only change.
	moduleBytes []byte

	// logToken is the current wrapper's ingestion token, read by the
	// write_logs callback to capture the token with each batch.
	logToken atomic.Value // string

	matStore MaterializationStore

	applyFilter  *seenFilter
	schemaFilter *seenFilter
	// seedFn overrides the per-instance random seed (tests pin it for
	// deterministic bucketing).
	seedFn func() int64
	// useInterpreter runs the module host on wazero's interpreter engine.
	useInterpreter bool
	// instancePoolSize bounds the module instance pool (0 = min(GOMAXPROCS, 4)).
	instancePoolSize int

	clientSDK *resolvertypes.Sdk

	stateMu                   sync.Mutex
	cancelFunc                context.CancelFunc
	wg                        sync.WaitGroup
	modulePollInterval        time.Duration
	logFlushInterval          time.Duration
	initializationTimeout     time.Duration
	initialRetryInterval      time.Duration
	ready                     atomic.Bool
	lifecycleEvents           chan openfeature.Event
	disableExposureCollection bool

	// Event tracking (optional — nil when no event WASM is provided)
	eventTracker eventTracking
	eventsClient events.EventsServiceClient
	eventsConn   *grpc.ClientConn

	// Event publish failure accounting. Failures are counted and reported once
	// per eventPublishLogWindow attempts instead of logging every failed RPC.
	eventPublishAttempts atomic.Int64
	eventPublishFailures atomic.Int64
}

// Compile-time interface conformance checks
var (
	_ openfeature.FeatureProvider = (*LocalResolverProvider)(nil)
	_ openfeature.StateHandler    = (*LocalResolverProvider)(nil)
	_ openfeature.EventHandler    = (*LocalResolverProvider)(nil)
	_ openfeature.Tracker         = (*LocalResolverProvider)(nil)
)

// eventTracking is the subset of the WASM event tracker used by the provider.
// Declared as an interface so the publish/drain path can be exercised without a
// live WASM instance; the only production implementation is *et.EventTracker.
type eventTracking interface {
	TrackEvent(request *eventswasm.TrackEventRequest) error
	FlushEvents() (*eventswasm.FlushEventsResponse, error)
	Close() error
}

// Metadata returns the provider metadata
func (p *LocalResolverProvider) Metadata() openfeature.Metadata {
	return openfeature.Metadata{
		Name: "confidence-sdk-go-local",
	}
}

// BooleanEvaluation evaluates a boolean flag
func (p *LocalResolverProvider) BooleanEvaluation(
	ctx context.Context,
	flag string,
	defaultValue bool,
	evalCtx openfeature.FlattenedContext,
) openfeature.BoolResolutionDetail {
	return evaluate(p, ctx, flag, defaultValue, evalCtx)
}

// StringEvaluation evaluates a string flag
func (p *LocalResolverProvider) StringEvaluation(
	ctx context.Context,
	flag string,
	defaultValue string,
	evalCtx openfeature.FlattenedContext,
) openfeature.StringResolutionDetail {
	return evaluate(p, ctx, flag, defaultValue, evalCtx)
}

// FloatEvaluation evaluates a float flag
func (p *LocalResolverProvider) FloatEvaluation(
	ctx context.Context,
	flag string,
	defaultValue float64,
	evalCtx openfeature.FlattenedContext,
) openfeature.FloatResolutionDetail {
	return evaluate(p, ctx, flag, defaultValue, evalCtx)
}

// IntEvaluation evaluates an int flag
func (p *LocalResolverProvider) IntEvaluation(
	ctx context.Context,
	flag string,
	defaultValue int64,
	evalCtx openfeature.FlattenedContext,
) openfeature.IntResolutionDetail {
	return evaluate(p, ctx, flag, defaultValue, evalCtx)
}

func (p *LocalResolverProvider) ObjectEvaluation(
	ctx context.Context,
	flag string,
	defaultValue interface{},
	evalCtx openfeature.FlattenedContext,
) openfeature.InterfaceResolutionDetail {
	return evaluate(p, ctx, flag, defaultValue, evalCtx)
}

// resolveOutcome is the result of driving one resolve through the module,
// including any suspend/resume round-trips the materialization store needed.
type resolveOutcome struct {
	host     *cm.Host
	envelope *cm.Envelope
	response *resolver.ResolveFlagsResponse
	// latency is measured from the start of the resolve through the final
	// resume and reported to the module's latency metrics.
	latency     time.Duration
	clientIndex int32
}

// resolveFlags drives a ResolveFlagsRequest through the live module,
// resuming suspensions from the materialization store when one is
// configured. It holds the host read lock for the whole flow so a module
// replacement cannot destroy the host under an in-flight call.
//
// Callers must hold p.hostMu.RLock until they are done with the outcome
// (envelope and host), so a replaced host is never closed under them.
func (p *LocalResolverProvider) resolveFlags(request *resolver.ResolveFlagsRequest) (*resolveOutcome, error) {
	start := time.Now()

	host := p.host
	if host == nil {
		return nil, errNoModule
	}

	result, suspended, err := host.StartResolve(request)
	if err != nil {
		return nil, err
	}

	// One suspend at most: the module asks for materializations only the
	// host's storage can provide. With no store configured, a resolve that
	// needs materializations fails with the documented actionable error
	// instead of resuming with an empty set: which record the module would
	// want next is the host's business, and the message tells the operator
	// what to configure.
	if suspended != nil {
		materializations := &flagswasm.Materialization{}
		if len(suspended.ToRead()) > 0 {
			if p.matStore == nil {
				host.Discard(suspended)
				return nil, fmt.Errorf("flag '%s' requires materializations; configure a materialization store", requestFlagName(request))
			}
			records, readErr := p.readMaterializations(suspended.ToRead())
			if readErr != nil {
				host.Discard(suspended)
				return nil, fmt.Errorf("failed to read materializations: %w", readErr)
			}
			materializations.Records = records
		}
		result, err = host.ResumeResolve(suspended, materializations)
		if err != nil {
			return nil, err
		}
		if result == nil {
			return nil, errors.New("no result after resume")
		}
	}

	// Persist materializations the module produced; a slow or failing store
	// must never fail the resolve.
	if len(result.MaterializationToWrite) > 0 {
		p.storeMaterializations(result.MaterializationToWrite)
	}

	return &resolveOutcome{
		host:        host,
		envelope:    result.Envelope,
		response:    result.Response,
		latency:     time.Since(start),
		clientIndex: result.ClientIndex,
	}, nil
}

// readMaterializations looks up the requested records in the store.
func (p *LocalResolverProvider) readMaterializations(toRead []*flagswasm.Materialization_Record) ([]*flagswasm.Materialization_Record, error) {
	readOps := materializationRecordsToReadOps(toRead)
	results, err := p.matStore.Read(context.Background(), readOps)
	if err != nil {
		var unsupported *MaterializationNotSupportedError
		if errors.As(err, &unsupported) {
			// Absent records count as absent: resolve without them.
			return nil, nil
		}
		return nil, err
	}
	return readResultsToMaterializationRecords(results), nil
}

// storeMaterializations hands the write batch to the store asynchronously.
func (p *LocalResolverProvider) storeMaterializations(records []*flagswasm.Materialization_Record) {
	writeOps := make([]WriteOp, len(records))
	for i, record := range records {
		writeOps[i] = newWriteOpVariant(
			record.GetMaterialization(),
			record.GetUnit(),
			record.GetRule(),
			record.GetVariant(),
		)
	}
	go func() {
		if err := p.matStore.Write(context.Background(), writeOps); err != nil {
			if _, ok := err.(*MaterializationNotSupportedError); !ok {
				p.logger.Debug("failed to persist materializations", "error", err)
			}
		}
	}()
}

// materializationRecordsToReadOps converts the module's records to store
// read operations. Records with a rule are variant reads; others are
// inclusion reads.
func materializationRecordsToReadOps(records []*flagswasm.Materialization_Record) []ReadOp {
	ops := make([]ReadOp, len(records))
	for i, record := range records {
		if record.GetRule() != "" {
			ops[i] = newReadOpVariant(record.GetMaterialization(), record.GetUnit(), record.GetRule())
		} else {
			ops[i] = newReadOpInclusion(record.GetMaterialization(), record.GetUnit())
		}
	}
	return ops
}

// readResultsToMaterializationRecords converts store results back to the
// module's records. Absent answers are omitted (absence = not included).
func readResultsToMaterializationRecords(results []ReadResult) []*flagswasm.Materialization_Record {
	var records []*flagswasm.Materialization_Record
	for _, result := range results {
		switch v := result.(type) {
		case *ReadResultVariant:
			if v.Variant() != nil && *v.Variant() != "" {
				records = append(records, &flagswasm.Materialization_Record{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
					Rule:            v.Rule(),
					Variant:         *v.Variant(),
				})
			}
		case *ReadResultInclusion:
			if v.Included() {
				records = append(records, &flagswasm.Materialization_Record{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
				})
			}
		}
	}
	return records
}

var errNoModule = errors.New("no module loaded")

// generic evaluation not a member since members can't be generic
func evaluate[T any](
	p *LocalResolverProvider,
	ctx context.Context,
	flag string,
	defaultValue T,
	evalCtx openfeature.FlattenedContext,
) openfeature.GenericResolutionDetail[T] {
	if !p.ready.Load() {
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewProviderNotReadyResolutionError("provider not initialized"),
			},
		}
	}

	// apply=false covers both provider DisableExposureCollection and
	// per-eval _confidence_skip_apply.
	apply := !p.disableExposureCollection
	if skip, ok := evalCtx["_confidence_skip_apply"]; ok {
		if b, ok := skip.(bool); ok && b {
			apply = false
		}
		delete(evalCtx, "_confidence_skip_apply")
	}

	processedCtx := processTargetingKey(evalCtx)
	protoCtx, err := flattenedContextToProto(processedCtx)
	if err != nil {
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewInvalidContextResolutionError(err.Error()),
			},
		}
	}

	request := &resolver.ResolveFlagsRequest{
		Flags:             []string{"flags/" + parseFlagName(flag)},
		EvaluationContext: protoCtx,
		ClientSecret:      p.clientSecret,
		Apply:             apply,
		Sdk:               p.clientSDK,
	}

	p.hostMu.RLock()
	defer p.hostMu.RUnlock()
	outcome, err := p.resolveFlags(request)
	if err != nil {
		var rejected *cm.ResolveRejectedError
		if errors.As(err, &rejected) && rejected.IsUnauthenticated() {
			return openfeature.GenericResolutionDetail[T]{
				Value: defaultValue,
				ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
					Reason:          openfeature.ErrorReason,
					ResolutionError: openfeature.NewGeneralResolutionError("module does not accept this client credential"),
				},
			}
		}
		p.logger.Error("Failed to resolve flag", "flag", flag, "error", err)
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewGeneralResolutionError(err.Error()),
			},
		}
	}
	defer outcome.envelope.Close()

	// Report the resolve's latency to the module's per-client metrics.
	outcome.host.ObserveResolveLatency(outcome.clientIndex, outcome.latency.Microseconds())

	// The evaluation step (key splitting, path walking, default merging,
	// type checks and error mapping) belongs to the module. Scalar defaults
	// are handed to the module so error paths echo them; object defaults
	// pass a null default and apply the documented struct/map merge
	// semantics locally, exactly as previous provider versions did.
	var defaultValueProto *structpb.Value
	var localUnmarshal bool
	switch any(defaultValue).(type) {
	case bool, string, float64, int64, int:
		proto, err := goValueToProtoValue(defaultValue)
		if err != nil {
			return openfeature.GenericResolutionDetail[T]{
				Value: defaultValue,
				ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
					Reason:          openfeature.ErrorReason,
					ResolutionError: openfeature.NewTypeMismatchResolutionError(err.Error()),
				},
			}
		}
		defaultValueProto = proto
	default:
		defaultValueProto = structpb.NewNullValue()
		localUnmarshal = true
	}
	details, err := outcome.host.Evaluate(outcome.envelope, &flagswasm.EvaluateRequest{
		FlagKey:      flag,
		DefaultValue: defaultValueProto,
	})
	if err != nil {
		p.logger.Error("Failed to evaluate flag", "flag", flag, "error", err)
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewGeneralResolutionError(err.Error()),
			},
		}
	}

	if code := mapErrorCode(details.GetErrorCode()); code != nil {
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: *code,
			},
		}
	}

	// The flag resolved with a per-flag reason that the evaluate step kept;
	// a materialization requirement surfaces here with the reason intact.
	if details.GetReason() == resolver.ResolveReason_RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED {
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewGeneralResolutionError(fmt.Sprintf("flag '%s' requires materializations; configure a materialization store", flag)),
			},
		}
	}

	var value T
	if localUnmarshal {
		value, err = unmarshalProto(details.GetValue(), defaultValue, nil)
	} else {
		value, err = protoValueToGo[T](details.GetValue())
	}
	if err != nil {
		return openfeature.GenericResolutionDetail[T]{
			Value: defaultValue,
			ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
				Reason:          openfeature.ErrorReason,
				ResolutionError: openfeature.NewTypeMismatchResolutionError(err.Error()),
			},
		}
	}

	return openfeature.GenericResolutionDetail[T]{
		Value: value,
		ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
			Variant:         details.GetVariant(),
			ResolutionError: openfeature.ResolutionError{},
			Reason:          mapResolveReasonToOpenFeature(details.GetReason()),
		},
	}
}

// mapErrorCode maps the module's evaluation error codes onto OpenFeature
// resolution errors; nil means success.
func mapErrorCode(code flagswasm.ErrorCode) *openfeature.ResolutionError {
	switch code {
	case flagswasm.ErrorCode_ERROR_CODE_UNSPECIFIED:
		return nil
	case flagswasm.ErrorCode_ERROR_CODE_PROVIDER_NOT_READY:
		e := openfeature.NewProviderNotReadyResolutionError("module not ready")
		return &e
	case flagswasm.ErrorCode_ERROR_CODE_FLAG_NOT_FOUND:
		e := openfeature.NewFlagNotFoundResolutionError("flag not found for this credential")
		return &e
	case flagswasm.ErrorCode_ERROR_CODE_TYPE_MISMATCH:
		e := openfeature.NewTypeMismatchResolutionError("resolved value does not match the requested type")
		return &e
	default: // PROVIDER_FATAL, GENERAL, CYCLE_DETECTED
		e := openfeature.NewGeneralResolutionError("flag evaluation failed")
		return &e
	}
}

// mapResolveReasonToOpenFeature converts Confidence resolve reasons to
// OpenFeature reasons.
func mapResolveReasonToOpenFeature(reason resolvertypes.ResolveReason) openfeature.Reason {
	switch reason {
	case resolvertypes.ResolveReason_RESOLVE_REASON_MATCH:
		return openfeature.TargetingMatchReason
	case resolvertypes.ResolveReason_RESOLVE_REASON_NO_SEGMENT_MATCH:
		return openfeature.DefaultReason
	case resolvertypes.ResolveReason_RESOLVE_REASON_FLAG_ARCHIVED:
		return openfeature.DisabledReason
	case resolvertypes.ResolveReason_RESOLVE_REASON_TARGETING_KEY_ERROR:
		return openfeature.ErrorReason
	case resolvertypes.ResolveReason_RESOLVE_REASON_ERROR:
		return openfeature.ErrorReason
	case resolvertypes.ResolveReason_RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE:
		return openfeature.ErrorReason
	default:
		return openfeature.UnknownReason
	}
}

// SnapshotConfig holds options for GetPrometheusMetrics.
//
// Experimental: this API is subject to change.
type SnapshotConfig struct {
	// BucketsPerDecade is retained for API compatibility. Module metrics
	// snapshots choose their own bucket density; the field is ignored.
	BucketsPerDecade uint32
	// OpenMetrics switches output to OpenMetrics text format.
	// When false (default), Prometheus exposition format is used.
	OpenMetrics bool
}

// GetPrometheusMetrics returns the module's metrics as Prometheus or
// OpenMetrics text. The snapshot is module-wide (not per instance) and
// reflects all resolves served by this provider.
//
// Experimental: this API is subject to change.
func (p *LocalResolverProvider) GetPrometheusMetrics(config SnapshotConfig) string {
	// Hold the read lock for the snapshot: a module swap must not destroy
	// the host under the metrics call.
	p.hostMu.RLock()
	defer p.hostMu.RUnlock()
	host := p.host
	if host == nil {
		return ""
	}
	text, err := host.MetricsSnapshot(config.OpenMetrics)
	if err != nil {
		p.logger.Warn("metrics snapshot failed", "error", err)
		return ""
	}
	return text
}

// Resolve resolves multiple flags for the given context. If flagNames is
// empty, all flags available to the client are resolved. When apply is true,
// exposure events are recorded immediately. When apply is false, the response
// carries a resolve_token that must be passed to ApplyFlags later to record
// exposures. With DisableExposureCollection configured no exposure token is
// returned.
//
// Returns an error if the provider has not been initialized (Init not
// called), the evaluation context cannot be converted, or the module fails
// to resolve.
func (p *LocalResolverProvider) Resolve(
	ctx context.Context,
	evalCtx openfeature.FlattenedContext,
	flagNames []string,
	apply bool,
) (*resolver.ResolveFlagsResponse, error) {
	if !p.ready.Load() {
		return nil, fmt.Errorf("provider not initialized")
	}

	requestFlags := make([]string, len(flagNames))
	for i, name := range flagNames {
		requestFlags[i] = "flags/" + name
	}

	protoCtx, err := flattenedContextToProto(processTargetingKey(evalCtx))
	if err != nil {
		return nil, fmt.Errorf("failed to convert context: %w", err)
	}

	p.hostMu.RLock()
	defer p.hostMu.RUnlock()
	outcome, err := p.resolveFlags(&resolver.ResolveFlagsRequest{
		Flags:             requestFlags,
		EvaluationContext: protoCtx,
		ClientSecret:      p.clientSecret,
		Apply:             apply,
		Sdk:               p.clientSDK,
	})
	if err != nil {
		return nil, err
	}
	defer outcome.envelope.Close()
	outcome.host.ObserveResolveLatency(outcome.clientIndex, outcome.latency.Microseconds())
	return outcome.response, nil
}

// ApplyFlags records exposure events for flags previously resolved with
// apply=false. The request must contain the resolve_token from the original
// resolve response. With DisableExposureCollection configured, ApplyFlags is
// a no-op.
//
// Returns an error if the provider has not been initialized. Non-zero
// statuses from the module are logged best-effort and not surfaced.
func (p *LocalResolverProvider) ApplyFlags(
	request *resolver.ApplyFlagsRequest,
) error {
	if !p.ready.Load() {
		return fmt.Errorf("provider not initialized")
	}
	if p.disableExposureCollection {
		return nil
	}

	p.hostMu.RLock()
	defer p.hostMu.RUnlock()
	host := p.host
	if host == nil {
		return errNoModule
	}

	req := &resolver.ApplyFlagsRequest{
		Flags:        request.GetFlags(),
		ResolveToken: request.GetResolveToken(),
		SendTime:     request.GetSendTime(),
		ClientSecret: p.clientSecret,
		Sdk:          request.GetSdk(),
	}
	if req.Sdk == nil {
		req.Sdk = p.clientSDK
	}
	status, err := host.ApplyFlags(req)
	if err != nil {
		p.logger.Warn("Failed to apply flags", "error", err)
		return nil
	}
	if status != cm.ApplyOK {
		// Best-effort logging failures: never surface as evaluation errors.
		p.logger.Warn("apply_flags reported a status", "status", status.String())
	}
	return nil
}

// Hooks returns provider hooks (none for this implementation)
func (p *LocalResolverProvider) Hooks() []openfeature.Hook {
	return []openfeature.Hook{}
}

// EventChannel reports provider lifecycle changes to the OpenFeature SDK.
func (p *LocalResolverProvider) EventChannel() <-chan openfeature.Event {
	return p.lifecycleEvents
}

func (p *LocalResolverProvider) emitLifecycleEvent(
	eventType openfeature.EventType,
	message string,
	errorCode openfeature.ErrorCode,
) {
	event := openfeature.Event{
		ProviderName: p.Metadata().Name,
		EventType:    eventType,
		ProviderEventDetails: openfeature.ProviderEventDetails{
			Message:   message,
			ErrorCode: errorCode,
		},
	}
	select {
	case p.lifecycleEvents <- event:
	default:
		p.logger.Warn("Dropping provider lifecycle event", "event_type", eventType)
	}
}

// Init initializes the provider (part of StateHandler interface): it fetches
// the credential's compiled module, validates and instantiates it, then
// starts the poll loop that keeps it fresh and the flush timer that keeps
// low-volume credentials observable. Until the first module is loaded,
// evaluations return the caller's default with PROVIDER_NOT_READY.
func (p *LocalResolverProvider) Init(evaluationContext openfeature.EvaluationContext) (err error) {
	ctx, cancel := context.WithCancel(context.Background())
	p.stateMu.Lock()
	p.ready.Store(false)
	p.cancelFunc = cancel
	p.wg.Add(1)
	p.stateMu.Unlock()
	defer p.wg.Done()
	defer func() {
		if r := recover(); r != nil {
			cancel()
			err = &openfeature.ProviderInitError{
				ErrorCode: openfeature.ProviderFatalCode,
				Message:   fmt.Sprintf("Init panicked: %v", r),
			}
		}
	}()

	if p.fetcher == nil {
		cancel()
		return fmt.Errorf("module fetcher is nil, cannot initialize")
	}

	startupCtx, startupCancel := context.WithTimeout(ctx, p.initializationTimeout)
	defer startupCancel()

	install := func() error {
		fetched, err := p.fetcher.Fetch(startupCtx)
		if err != nil {
			return err
		}
		return p.installModule(fetched)
	}

	var lastErr error
	finishStartup := func() error {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		// Keep retrying in the background; the provider serves defaults
		// until a module loads.
		p.startScheduledTasks(ctx)
		p.logger.Warn("Initial module load timed out; serving default values while recovery continues", "error", lastErr)
		return fmt.Errorf("provider initialization timed out after %s: %w", p.initializationTimeout, startupCtx.Err())
	}
	for {
		lastErr = install()
		if startupCtx.Err() != nil {
			return finishStartup()
		}
		if lastErr == nil {
			if !p.transitionToReady(ctx, false) {
				return ctx.Err()
			}
			p.startScheduledTasks(ctx)
			p.logger.Info("Provider initialized successfully")
			return nil
		}
		if !isModuleNotPublished(lastErr) && !isModuleUnchanged(lastErr) {
			p.logger.Error("Initial module load failed", "error", lastErr)
		}

		retryTimer := time.NewTimer(p.initialRetryInterval)
		select {
		case <-retryTimer.C:
			continue
		case <-startupCtx.Done():
			retryTimer.Stop()
			return finishStartup()
		}
	}
}

// installModule validates a fetched artifact and swaps the live host. A
// wrapper whose module bytes match the live module is a token-only change:
// the new token takes effect for new batches without re-instantiation. A
// module that fails validation keeps the previous host serving; only the
// initial load (no previous host) propagates the error.
func (p *LocalResolverProvider) installModule(fetched *FetchedModule) error {
	p.hostMu.RLock()
	sameModule := p.host != nil && bytesEqual(p.moduleBytes, fetched.Module)
	p.hostMu.RUnlock()
	if sameModule {
		// Wrapper-only change: new log token, same module.
		p.logToken.Store(fetched.LogToken)
		p.logger.Info("Module wrapper refreshed", "log_token_changed", true)
		return nil
	}

	module, err := cm.LoadModule(fetched.Module, p.clientSecret)
	if err != nil {
		return fmt.Errorf("rejected module: %w", err)
	}

	host, err := cm.NewHost(module, cm.HostConfig{
		LogSink: p.moduleLogSink,
		Seed:    p.seedFn,
		ApplySeen: func(fingerprint int64) bool {
			return p.applyFilter.Seen(fingerprint)
		},
		SchemaSeen: func(fingerprint int64) bool {
			return p.schemaFilter.Seen(fingerprint)
		},
		Logger:         p.logger,
		UseInterpreter: p.useInterpreter,
		InstanceCap:    p.instancePoolSize,
	})
	if err != nil {
		return fmt.Errorf("failed to instantiate module: %w", err)
	}

	p.hostMu.Lock()
	old := p.host
	p.host = host
	p.moduleBytes = fetched.Module
	p.logToken.Store(fetched.LogToken)
	p.hostMu.Unlock()

	// Stop routing to the old host (done above), wait for in-flight calls
	// to drain, then retire the old instances while its memory is still
	// alive, and release it. A failed validation above keeps the old host
	// serving: it is only replaced here, after the new one is live.
	if old != nil {
		p.wg.Add(1)
		go func() {
			defer p.wg.Done()
			old.Close()
		}()
	}
	return nil
}

// moduleFetcher is the artifact source the provider polls.
type moduleFetcher interface {
	Fetch(ctx context.Context) (*FetchedModule, error)
}

// moduleLogSink is the write_logs callback target: the token is captured with
// the batch at delivery time and the batch is handed to the forwarder, which
// never blocks the callback.
func (p *LocalResolverProvider) moduleLogSink(body []byte) {
	token, _ := p.logToken.Load().(string)
	p.forwarder.Enqueue(body, token)
}

// transitionToReady flips the provider to READY once, reporting recovery.
func (p *LocalResolverProvider) transitionToReady(ctx context.Context, reportRecovery bool) bool {
	p.stateMu.Lock()
	defer p.stateMu.Unlock()

	if ctx.Err() != nil {
		return false
	}
	if becameReady := !p.ready.Swap(true); becameReady && reportRecovery {
		p.logger.Info("Provider recovered and is now ready")
		p.emitLifecycleEvent(openfeature.ProviderReady, "Provider recovered and is now ready", "")
	}
	return true
}

// Shutdown closes the provider and cleans up resources (part of StateHandler
// interface). Log batches still queued are drained with a bounded deadline.
func (p *LocalResolverProvider) Shutdown() {
	ctx := context.Background()
	p.stateMu.Lock()
	cancel := p.cancelFunc
	p.cancelFunc = nil
	if cancel != nil {
		cancel()
	}
	p.ready.Store(false)
	p.stateMu.Unlock()

	if p.logger != nil {
		p.logger.Info("Shutting down provider")
	}

	// Wait for background goroutines to exit.
	p.wg.Wait()

	// Drain and close the event tracker BEFORE the final log flush: event
	// delivery outcomes ride on the next log batch.
	if p.eventTracker != nil {
		drainCtx, drainCancel := context.WithTimeout(ctx, 3*time.Second)
		p.drainEvents(drainCtx)
		drainCancel()
		if err := p.eventTracker.Close(); err != nil && p.logger != nil {
			p.logger.Warn("Failed to close event tracker", "error", err)
		}
		if p.eventsConn != nil {
			if err := p.eventsConn.Close(); err != nil && p.logger != nil {
				p.logger.Warn("Failed to close events service channel", "error", err)
			}
			p.eventsConn = nil
		}
	}

	// Final log flush so the last batches are queued before the forwarder
	// drains. The lock is held for the call, then released before the host
	// is retired below.
	p.hostMu.RLock()
	host := p.host
	if host != nil {
		if err := host.FlushLogs(); err != nil {
			p.logger.Warn("final log flush failed", "error", err)
		}
	}
	p.hostMu.RUnlock()

	// Drain queued batches, then stop the forwarder.
	p.forwarder.Shutdown(3 * time.Second)

	// Retire the module instance and release the shared memory.
	if host != nil {
		host.Close()
		p.hostMu.Lock()
		p.host = nil
		p.hostMu.Unlock()
	}

	if p.logger != nil {
		p.logger.Info("Provider has been shut down")
	}
}

// startScheduledTasks starts the background poll loop, the log flush timer
// and the event flush timer.
func (p *LocalResolverProvider) startScheduledTasks(parentCtx context.Context) {
	// Module poll loop.
	p.wg.Add(1)
	go func() {
		defer p.wg.Done()
		nextInterval := func() time.Duration {
			if p.ready.Load() {
				return p.modulePollInterval
			}
			return p.initialRetryInterval
		}
		timer := time.NewTimer(nextInterval())
		defer timer.Stop()

		for {
			select {
			case <-timer.C:
				fetched, err := p.fetcher.Fetch(parentCtx)
				if parentCtx.Err() != nil {
					return
				}
				switch {
				case err == nil:
					if err := p.installModule(fetched); err != nil {
						p.logger.Error("Failed to install refreshed module; keeping the previous module", "error", err)
					}
				case isModuleNotPublished(err):
					// No module published (yet, or no longer): keep
					// serving whatever is live.
				case isModuleUnchanged(err):
					// Nothing to do.
				default:
					p.logger.Error("Module fetch failed", "error", err)
				}
				if !p.ready.Load() {
					p.transitionToReady(parentCtx, true)
				}
				timer.Reset(nextInterval())
			case <-parentCtx.Done():
				return
			}
		}
	}()

	// Log flush timer: low-volume credentials stay observable.
	p.wg.Add(1)
	go func() {
		defer p.wg.Done()
		ticker := time.NewTicker(p.logFlushInterval)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				if !p.ready.Load() {
					continue
				}
				// Hold the read lock for the flush: it may block on a busy
				// instance pool, and a module swap must not destroy the host
				// underneath it.
				p.hostMu.RLock()
				host := p.host
				if host != nil {
					if err := host.FlushLogs(); err != nil {
						p.logger.Error("Failed to flush logs", "error", err)
					}
				}
				p.hostMu.RUnlock()
			case <-parentCtx.Done():
				return
			}
		}
	}()

	// Event flushing (only when event tracking is enabled).
	if p.eventTracker != nil {
		p.wg.Add(1)
		go func() {
			defer p.wg.Done()
			ticker := time.NewTicker(p.logFlushInterval)
			defer ticker.Stop()

			for {
				select {
				case <-ticker.C:
					p.flushAndPublishEvents(parentCtx)
				case <-parentCtx.Done():
					return
				}
			}
		}()
	}
}

// requestFlagName returns the bare flag name of a resolve request, for
// error messages.
func requestFlagName(request *resolver.ResolveFlagsRequest) string {
	for _, f := range request.GetFlags() {
		if name := strings.TrimPrefix(f, "flags/"); name != "" {
			return name
		}
	}
	return "unknown"
}

// parseFlagName splits the flag portion of a dotted key; the value path is
// handled by the module's evaluate step.
func parseFlagName(key string) string {
	for i := 0; i < len(key); i++ {
		if key[i] == '.' {
			return key[:i]
		}
	}
	return key
}

// processTargetingKey converts "targetingKey" to "targeting_key" in the context.
func processTargetingKey(evalCtx openfeature.FlattenedContext) openfeature.FlattenedContext {
	newEvalContext := make(openfeature.FlattenedContext)
	for k, v := range evalCtx {
		newEvalContext[k] = v
	}

	if targetingKey, exists := evalCtx["targetingKey"]; exists {
		newEvalContext["targeting_key"] = targetingKey
		delete(newEvalContext, "targetingKey")
	}

	return newEvalContext
}

// flattenedContextToProto converts OpenFeature FlattenedContext to protobuf Struct.
func flattenedContextToProto(ctx openfeature.FlattenedContext) (*structpb.Struct, error) {
	fields := make(map[string]*structpb.Value)

	for key, value := range ctx {
		protoValue, err := goValueToProto(value)
		if err != nil {
			return nil, fmt.Errorf("failed to convert field '%s': %w", key, err)
		}
		fields[key] = protoValue
	}

	return &structpb.Struct{Fields: fields}, nil
}

// goValueToProto converts a Go value to a protobuf Value.
func goValueToProto(value interface{}) (*structpb.Value, error) {
	switch v := value.(type) {
	case nil:
		return structpb.NewNullValue(), nil
	case bool:
		return structpb.NewBoolValue(v), nil
	case int:
		return structpb.NewNumberValue(float64(v)), nil
	case int64:
		return structpb.NewNumberValue(float64(v)), nil
	case float64:
		return structpb.NewNumberValue(v), nil
	case string:
		return structpb.NewStringValue(v), nil
	case []interface{}:
		values := make([]*structpb.Value, len(v))
		for i, item := range v {
			val, err := goValueToProto(item)
			if err != nil {
				return nil, err
			}
			values[i] = val
		}
		return structpb.NewListValue(&structpb.ListValue{Values: values}), nil
	case map[string]interface{}:
		fields := make(map[string]*structpb.Value)
		for key, val := range v {
			protoVal, err := goValueToProto(val)
			if err != nil {
				return nil, err
			}
			fields[key] = protoVal
		}
		return structpb.NewStructValue(&structpb.Struct{Fields: fields}), nil
	default:
		return nil, fmt.Errorf("unsupported type: %T", v)
	}
}

// goValueToProtoValue converts a typed default to a protobuf Value for the
// module's evaluate request.
func goValueToProtoValue[T any](value T) (*structpb.Value, error) {
	return goValueToProto(any(value))
}

// protoValueToGo converts the module's evaluated Value back to the caller's
// type. The module returns the caller's default on every error path, so a
// conversion failure here means the resolved value does not fit the
// requested type.
func protoValueToGo[T any](value *structpb.Value) (T, error) {
	var zero T
	if value == nil {
		return zero, nil
	}
	var target T
	switch any(target).(type) {
	case bool:
		b, ok := value.GetKind().(*structpb.Value_BoolValue)
		if !ok {
			return zero, fmt.Errorf("expected a bool value, got %s", kindName(value))
		}
		return any(b.BoolValue).(T), nil
	case string:
		s, ok := value.GetKind().(*structpb.Value_StringValue)
		if !ok {
			return zero, fmt.Errorf("expected a string value, got %s", kindName(value))
		}
		return any(s.StringValue).(T), nil
	case float64:
		n, ok := value.GetKind().(*structpb.Value_NumberValue)
		if !ok {
			return zero, fmt.Errorf("expected a number value, got %s", kindName(value))
		}
		return any(n.NumberValue).(T), nil
	case int64:
		n, ok := value.GetKind().(*structpb.Value_NumberValue)
		if !ok {
			return zero, fmt.Errorf("expected a number value, got %s", kindName(value))
		}
		if n.NumberValue != float64(int64(n.NumberValue)) {
			return zero, fmt.Errorf("value %v is not a whole number", n.NumberValue)
		}
		return any(int64(n.NumberValue)).(T), nil
	case int:
		n, ok := value.GetKind().(*structpb.Value_NumberValue)
		if !ok {
			return zero, fmt.Errorf("expected a number value, got %s", kindName(value))
		}
		if n.NumberValue != float64(int(n.NumberValue)) {
			return zero, fmt.Errorf("value %v is not a whole number", n.NumberValue)
		}
		return any(int(n.NumberValue)).(T), nil
	default:
		// Object evaluation: the module already merged defaults into
		// nulls; the interface value is the decoded structure.
		return value.AsInterface().(T), nil
	}
}

func kindName(value *structpb.Value) string {
	switch value.GetKind().(type) {
	case *structpb.Value_NullValue:
		return "null"
	case *structpb.Value_NumberValue:
		return "number"
	case *structpb.Value_StringValue:
		return "string"
	case *structpb.Value_BoolValue:
		return "bool"
	case *structpb.Value_StructValue:
		return "struct"
	case *structpb.Value_ListValue:
		return "list"
	default:
		return "unknown"
	}
}

func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// eventsGrpcTarget is the gRPC target for the Confidence events service.
const eventsGrpcTarget = "edge-grpc.spotify.com:443"

// eventsPublishTimeout bounds a single PublishEvents RPC.
const eventsPublishTimeout = 30 * time.Second

// eventsRetryServiceConfig is the gRPC service config applied to the events
// channel: transparent retries of transient UNAVAILABLE failures with
// exponential backoff. Kept in sync with the other Confidence SDKs.
const eventsRetryServiceConfig = `{
  "methodConfig": [
    {
      "name": [{"service": "confidence.events.v1.EventsService"}],
      "retryPolicy": {
        "maxAttempts": 3,
        "initialBackoff": "1s",
        "maxBackoff": "10s",
        "backoffMultiplier": 2.0,
        "retryableStatusCodes": ["UNAVAILABLE"]
      }
    }
  ]
}`

// eventPublishLogWindow is how many publish attempts are aggregated before
// failures are reported, mirroring the forwarder's failure accounting.
const eventPublishLogWindow = 10

// maxDrainBatches bounds the number of flush/publish rounds performed on
// shutdown. A single flush is capped by the event guest's byte budget, so a
// backlog needs several rounds to drain; the bound keeps shutdown from
// spinning forever when the events service is unreachable (publish failures
// are swallowed).
const maxDrainBatches = 100

// eventsSdk identifies this SDK to the events service.
var eventsSdk = &events.Sdk{
	Sdk:     &events.Sdk_Id{Id: events.SdkId_SDK_ID_GO_LOCAL_PROVIDER},
	Version: Version,
}

// defaultLogger builds the provider's fallback logger.
func defaultLogger() *slog.Logger {
	return slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))
}

// Track sends a tracking event to the event engine, implementing the
// OpenFeature Tracker interface. The event is buffered internally and published
// in batches by the background flush goroutine. This is a no-op if event
// tracking was not enabled via WithEventTracking.
//
// The numeric value from details is only attached when non-zero: the
// OpenFeature TrackingEventDetails zero value is indistinguishable from an
// explicitly-set 0, so events without a measurement are sent without a value.
func (p *LocalResolverProvider) Track(
	ctx context.Context,
	trackingEventName string,
	evalCtx openfeature.EvaluationContext,
	details openfeature.TrackingEventDetails,
) {
	if p.eventTracker == nil {
		return
	}

	// Attributes() returns a fresh copy, so adding the targeting key here does
	// not mutate the caller's evaluation context.
	flatCtx := openfeature.FlattenedContext(evalCtx.Attributes())
	if targetingKey := evalCtx.TargetingKey(); targetingKey != "" {
		flatCtx[openfeature.TargetingKey] = targetingKey
	}

	var protoCtx *structpb.Struct
	if len(flatCtx) > 0 {
		var err error
		protoCtx, err = flattenedContextToProto(processTargetingKey(flatCtx))
		if err != nil {
			p.logger.Warn("Failed to convert context for event tracking", "error", err)
			return
		}
	}

	var protoData *structpb.Struct
	if attributes := details.Attributes(); len(attributes) > 0 {
		var err error
		protoData, err = structpb.NewStruct(attributes)
		if err != nil {
			p.logger.Warn("Failed to convert data for event tracking", "error", err)
			return
		}
	}

	// Go's TrackingEventDetails stores value as a plain float64 with no "is set"
	// flag, so an explicit 0 is indistinguishable from an unset value. Java
	// (Optional) and JS (number | undefined) can tell them apart and do forward
	// an explicit 0. We treat 0 as unset: always sending would attach a spurious
	// value: 0 to every event where the caller set none, which is far more
	// common than deliberately tracking a zero. See confidence-event-engine's
	// README ("Known provider differences").
	var value *float64
	if v := details.Value(); v != 0 {
		value = &v
	}

	request := &eventswasm.TrackEventRequest{
		EventName: trackingEventName,
		EventTime: timestamppb.Now(),
		Value:     value,
		Context:   protoCtx,
		Data:      protoData,
	}

	if err := p.eventTracker.TrackEvent(request); err != nil {
		p.logger.Warn("Failed to track event", "event", trackingEventName, "error", err)
	}
}

// flushAndPublishEvents retrieves buffered events from the WASM event engine
// and publishes them to the Confidence events service over gRPC. It returns the
// number of events flushed, which is 0 when the buffer was empty or the flush
// itself failed.
//
// Publish failures are swallowed (the events are already gone from the WASM
// buffer); they are counted and reported once per eventPublishLogWindow
// attempts.
func (p *LocalResolverProvider) flushAndPublishEvents(ctx context.Context) int {
	if p.eventTracker == nil {
		return 0
	}

	batch, err := p.eventTracker.FlushEvents()
	if err != nil {
		p.logger.Error("Failed to flush events from WASM", "error", err)
		return 0
	}

	if len(batch.Events) == 0 {
		return 0
	}

	rejected, err := p.publishEvents(ctx, batch)
	if err != nil {
		p.eventPublishFailures.Add(1)
		p.logger.Debug("Failed to publish events", "error", err)
	} else if rejected > 0 {
		p.logger.Debug("Events rejected on publish", "rejected", rejected)
	}

	if p.eventPublishAttempts.Add(1)%eventPublishLogWindow == 0 {
		if failures := p.eventPublishFailures.Swap(0); failures > 0 {
			p.logger.Warn("Event publish failures", "failures", failures, "window", eventPublishLogWindow)
		}
	}

	return len(batch.Events)
}

// drainEvents flushes and publishes buffered events repeatedly until the buffer
// is empty. A single flush is bounded by the event guest's byte budget, so one
// round is not enough to drain a backlog; the loop is bounded by
// maxDrainBatches because publish failures are swallowed and would otherwise
// let an unreachable events service spin forever.
func (p *LocalResolverProvider) drainEvents(ctx context.Context) {
	if p.eventTracker == nil {
		return
	}

	for i := 0; i < maxDrainBatches; i++ {
		if p.flushAndPublishEvents(ctx) == 0 {
			return
		}
		if ctx.Err() != nil {
			return
		}
	}

	p.logger.Warn("Event drain hit the batch limit on shutdown; dropping the rest",
		"max_drain_batches", maxDrainBatches)
}

// publishEvents wraps a flushed batch in a PublishEventsRequest and sends it to
// the Confidence events service.
func (p *LocalResolverProvider) publishEvents(ctx context.Context, batch *eventswasm.FlushEventsResponse) (int, error) {
	if p.eventsClient == nil {
		return 0, errors.New("events service client is not configured")
	}

	rpcCtx, cancel := context.WithTimeout(ctx, eventsPublishTimeout)
	defer cancel()

	request := &events.PublishEventsRequest{
		ClientSecret: p.clientSecret,
		Events:       batch.Events,
		SendTime:     timestamppb.Now(),
		Sdk:          eventsSdk,
	}

	response, err := p.eventsClient.PublishEvents(rpcCtx, request)
	if err != nil {
		return 0, fmt.Errorf("failed to publish events: %w", err)
	}

	rejected := len(response.GetErrors())
	for _, e := range response.GetErrors() {
		p.logger.Error("Event publish error",
			"index", e.GetIndex(),
			"reason", e.GetReason().String(),
			"message", e.GetMessage())
	}

	return rejected, nil
}
