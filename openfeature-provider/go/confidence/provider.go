// Package confidence is an OpenFeature provider that resolves Confidence flags locally, by running
// the credential's compiled resolver module (state-to-wasm) fetched from the Confidence CDN.
package confidence

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"runtime"
	"sync"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/cdn"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/engine"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/host"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

const providerName = "confidence-sdk-go-local"

// ExposureMode selects how exposure (apply) events are collected.
type ExposureMode = engine.ExposureMode

const (
	// ExposureDeduplicated logs each unit's exposure to a variant once (per dedup window). Default.
	ExposureDeduplicated = engine.ExposureDeduplicated
	// ExposureAll logs every exposure.
	ExposureAll = engine.ExposureAll
	// ExposureNone suppresses exposure events (error events are still sent).
	ExposureNone = engine.ExposureNone
)

// ProviderConfig configures a provider. Only ClientSecret and EncryptionKey are required.
type ProviderConfig struct {
	ClientSecret string
	// EncryptionKey is the credential's hex-encoded AES-256 key (64 hexadecimal characters).
	EncryptionKey string

	// MaterializationStore stores sticky assignments and materialized segments. Optional: flags
	// that need materializations resolve with an error without one.
	MaterializationStore MaterializationStore
	// UseRemoteMaterializationStore uses Confidence's hosted store (protobuf over HTTP). Requires
	// MaterializationStore nil.
	UseRemoteMaterializationStore bool

	// Instances bounds concurrent evaluations. Default: GOMAXPROCS.
	Instances int
	// MaxMemoryBytes caps the resolver's memory. Default 64 MiB.
	MaxMemoryBytes uint64
	// StatePollInterval is how often to check for flag state updates. Default 10s.
	StatePollInterval time.Duration
	// InitializationTimeout bounds Init's wait for the first state. Default 30s.
	InitializationTimeout time.Duration
	ExposureMode          ExposureMode
	// UseWasmInterpreter runs the resolver in wazero's interpreter instead of its compiler: much
	// slower, but safe with Go's profiling signals and preemption. Default false.
	UseWasmInterpreter bool

	// HTTPClient is used for all requests to Confidence. Default http.DefaultClient.
	HTTPClient *http.Client
	Logger     *slog.Logger
	// ResolverBaseURL is the base URL for module profile registration. Default https://resolver.confidence.dev.
	ResolverBaseURL string

	// Test-only overrides.
	cdnBaseURL, ingestBaseURL, remoteStoreURL string
	retryInterval                             time.Duration
}

// LocalResolverProvider implements openfeature.FeatureProvider with local resolution.
type LocalResolverProvider struct {
	engine      *engine.Engine
	store       host.MaterializationStore
	secret      string
	initTimeout time.Duration
	logger      *slog.Logger
	events      chan openfeature.Event
	shutdown    sync.Once
}

var (
	_ openfeature.FeatureProvider = (*LocalResolverProvider)(nil)
	_ openfeature.StateHandler    = (*LocalResolverProvider)(nil)
	_ openfeature.EventHandler    = (*LocalResolverProvider)(nil)
)

// NewProvider validates config and creates a provider. It doesn't touch the network: register it
// with OpenFeature (or call Init) to load flag state.
func NewProvider(ctx context.Context, config ProviderConfig) (*LocalResolverProvider, error) {
	if config.ClientSecret == "" {
		return nil, errors.New("ClientSecret is required")
	}
	key, err := cdn.ParseKey(config.EncryptionKey)
	if err != nil {
		return nil, err
	}
	if config.MaterializationStore != nil && config.UseRemoteMaterializationStore {
		return nil, errors.New("set either MaterializationStore or UseRemoteMaterializationStore, not both")
	}
	if config.Logger == nil {
		config.Logger = slog.Default()
	}
	if config.Instances <= 0 {
		config.Instances = runtime.GOMAXPROCS(0)
	}
	if config.InitializationTimeout <= 0 {
		config.InitializationTimeout = 30 * time.Second
	}
	store := config.MaterializationStore
	switch {
	case config.UseRemoteMaterializationStore:
		store = newRemoteMaterializationStore(config.remoteStoreURL, config.ClientSecret, config.HTTPClient)
	case store == nil:
		store = newUnsupportedMaterializationStore()
	}
	p := &LocalResolverProvider{
		store: storeAdapter{store, config.Logger}, secret: config.ClientSecret, initTimeout: config.InitializationTimeout,
		logger: config.Logger, events: make(chan openfeature.Event, 8),
	}
	p.engine, err = engine.New(ctx, engine.Config{
		ClientSecret: config.ClientSecret, EncryptionKey: key, HTTPClient: config.HTTPClient,
		CDNBaseURL: config.cdnBaseURL, IngestBaseURL: config.ingestBaseURL, ResolverBaseURL: config.ResolverBaseURL,
		Instances: config.Instances, MaxMemoryBytes: config.MaxMemoryBytes, Interpreter: config.UseWasmInterpreter,
		Exposure: config.ExposureMode, PollInterval: config.StatePollInterval, RetryInterval: config.retryInterval,
		Logger: config.Logger,
		OnReady: func() {
			p.emit(openfeature.ProviderReady, "flag state loaded", "")
		},
	})
	if err != nil {
		return nil, err
	}
	return p, nil
}

func (p *LocalResolverProvider) Metadata() openfeature.Metadata {
	return openfeature.Metadata{Name: providerName}
}

func (p *LocalResolverProvider) Hooks() []openfeature.Hook { return nil }

// EventChannel reports provider lifecycle changes to the OpenFeature SDK.
func (p *LocalResolverProvider) EventChannel() <-chan openfeature.Event { return p.events }

func (p *LocalResolverProvider) emit(t openfeature.EventType, msg string, code openfeature.ErrorCode) {
	select {
	case p.events <- openfeature.Event{ProviderName: providerName, EventType: t,
		ProviderEventDetails: openfeature.ProviderEventDetails{Message: msg, ErrorCode: code}}:
	default:
		p.logger.Warn("dropping provider lifecycle event", "event", t)
	}
}

// Init loads the first flag state, waiting up to InitializationTimeout. If that times out it
// returns an error but keeps trying in the background, and emits ProviderReady once loaded.
func (p *LocalResolverProvider) Init(openfeature.EvaluationContext) error {
	err := p.engine.Start(context.Background(), p.initTimeout)
	var term *engine.TerminalError
	if errors.As(err, &term) {
		return &openfeature.ProviderInitError{ErrorCode: openfeature.ProviderFatalCode, Message: err.Error()}
	}
	return err
}

// Shutdown flushes buffered flag logs and releases the resolver.
func (p *LocalResolverProvider) Shutdown() {
	p.shutdown.Do(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := p.engine.Close(ctx); err != nil {
			p.logger.Warn("provider shutdown", "error", err)
		}
	})
}

func (p *LocalResolverProvider) BooleanEvaluation(ctx context.Context, flag string, def bool, evalCtx openfeature.FlattenedContext) openfeature.BoolResolutionDetail {
	d := evaluate(p, ctx, flag, def, evalCtx)
	return openfeature.BoolResolutionDetail{Value: d.Value, ProviderResolutionDetail: d.ProviderResolutionDetail}
}

func (p *LocalResolverProvider) StringEvaluation(ctx context.Context, flag string, def string, evalCtx openfeature.FlattenedContext) openfeature.StringResolutionDetail {
	d := evaluate(p, ctx, flag, def, evalCtx)
	return openfeature.StringResolutionDetail{Value: d.Value, ProviderResolutionDetail: d.ProviderResolutionDetail}
}

func (p *LocalResolverProvider) FloatEvaluation(ctx context.Context, flag string, def float64, evalCtx openfeature.FlattenedContext) openfeature.FloatResolutionDetail {
	d := evaluate(p, ctx, flag, def, evalCtx)
	return openfeature.FloatResolutionDetail{Value: d.Value, ProviderResolutionDetail: d.ProviderResolutionDetail}
}

func (p *LocalResolverProvider) IntEvaluation(ctx context.Context, flag string, def int64, evalCtx openfeature.FlattenedContext) openfeature.IntResolutionDetail {
	d := evaluate(p, ctx, flag, def, evalCtx)
	return openfeature.IntResolutionDetail{Value: d.Value, ProviderResolutionDetail: d.ProviderResolutionDetail}
}

// ObjectEvaluation decodes the flag value into the type of def: a struct (fields matched by json
// tag, else snake_case name), a map with string keys, a pointer to either, or nil for the value's
// natural Go representation.
func (p *LocalResolverProvider) ObjectEvaluation(ctx context.Context, flag string, def any, evalCtx openfeature.FlattenedContext) openfeature.InterfaceResolutionDetail {
	d := evaluate(p, ctx, flag, def, evalCtx)
	return openfeature.InterfaceResolutionDetail{Value: d.Value, ProviderResolutionDetail: d.ProviderResolutionDetail}
}

func evaluate[T any](p *LocalResolverProvider, ctx context.Context, flag string, def T, evalCtx openfeature.FlattenedContext) openfeature.GenericResolutionDetail[T] {
	fail := func(err openfeature.ResolutionError) openfeature.GenericResolutionDetail[T] {
		return openfeature.GenericResolutionDetail[T]{Value: def, ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
			Reason: openfeature.ErrorReason, ResolutionError: err}}
	}
	defValue, schema, err := encodeDefault(def)
	if err != nil {
		return fail(openfeature.NewTypeMismatchResolutionError(err.Error()))
	}
	flagName, _, _ := cut(flag)
	req, err := p.request(evalCtx, []string{flagName}, evalCtx[skipApplyKey] != true)
	if err != nil {
		return fail(openfeature.NewInvalidContextResolutionError(err.Error()))
	}
	details, err := p.engine.Evaluate(ctx, req, []*wasmpb.EvaluateRequest{{FlagKey: flag, DefaultValue: defValue, ExpectedSchema: schema}}, p.store)
	if err != nil {
		return fail(p.resolveError(flagName, err))
	}
	d := details[0]
	if code := d.GetErrorCode(); code != wasmpb.ErrorCode_ERROR_CODE_UNSPECIFIED {
		return fail(resolutionError(code, d.GetErrorMessage()))
	}
	value, err := decode(d.GetValue(), any(def))
	if err != nil {
		return fail(openfeature.NewTypeMismatchResolutionError(err.Error()))
	}
	typed, _ := value.(T)
	return openfeature.GenericResolutionDetail[T]{Value: typed, ProviderResolutionDetail: openfeature.ProviderResolutionDetail{
		Variant: d.GetVariant(), Reason: reason(d.GetReason())}}
}

// Resolve resolves flags (names without the "flags/" prefix) and returns the full response. With
// apply false, exposure is deferred: pass the response's resolve token to ApplyFlags later. The
// token is neither encrypted nor authenticated; see the README before it leaves your backend.
func (p *LocalResolverProvider) Resolve(ctx context.Context, evalCtx openfeature.FlattenedContext, flagNames []string, apply bool) (*resolverpb.ResolveFlagsResponse, error) {
	req, err := p.request(evalCtx, flagNames, apply)
	if err != nil {
		return nil, err
	}
	return p.engine.Resolve(ctx, req, p.store)
}

// ApplyFlags records exposure for flags resolved with apply=false. The request carries the resolve
// token from that response. The client secret is filled in if empty. The token is trusted as is:
// pass only tokens your backend issued, or authenticated on their way back.
func (p *LocalResolverProvider) ApplyFlags(request *resolverpb.ApplyFlagsRequest) error {
	if request.GetClientSecret() == "" {
		request = proto.CloneOf(request)
		request.ClientSecret = p.secret
	}
	b, err := proto.Marshal(request)
	if err != nil {
		return err
	}
	return p.engine.ApplyFlags(context.Background(), b)
}

// SnapshotConfig holds options for GetPrometheusMetrics.
//
// Experimental: this API is subject to change.
type SnapshotConfig struct {
	// OpenMetrics switches the output to the OpenMetrics text format (default: Prometheus).
	OpenMetrics bool
}

// GetPrometheusMetrics returns the resolver's cumulative metrics in Prometheus (or OpenMetrics)
// text format, or "" before flag state is loaded.
//
// Experimental: this API is subject to change.
func (p *LocalResolverProvider) GetPrometheusMetrics(config SnapshotConfig) string {
	text, err := p.engine.Metrics(context.Background(), config.OpenMetrics)
	if err != nil {
		p.logger.Warn("metrics snapshot", "error", err)
	}
	return text
}

// skipApplyKey in an evaluation context, set to true, skips exposure for that evaluation (it resolves
// with apply false). It's an instruction to the provider, so it never reaches the module.
const skipApplyKey = "_confidence_skip_apply"

func (p *LocalResolverProvider) request(evalCtx openfeature.FlattenedContext, flagNames []string, apply bool) (*resolverpb.ResolveFlagsRequest, error) {
	fields := make(map[string]*structpb.Value, len(evalCtx))
	for k, v := range evalCtx {
		if k == skipApplyKey {
			continue
		}
		if k == openfeature.TargetingKey {
			k = "targeting_key"
		}
		value, err := structpb.NewValue(normalize(v))
		if err != nil {
			return nil, fmt.Errorf("evaluation context field %q: %w", k, err)
		}
		fields[k] = value
	}
	flags := make([]string, len(flagNames))
	for i, f := range flagNames {
		flags[i] = "flags/" + f
	}
	return &resolverpb.ResolveFlagsRequest{
		Flags: flags, EvaluationContext: &structpb.Struct{Fields: fields}, ClientSecret: p.secret, Apply: apply,
		Sdk: &resolverpb.Sdk{Sdk: &resolverpb.Sdk_Id{Id: resolverpb.SdkId_SDK_ID_GO_LOCAL_PROVIDER}, Version: Version},
	}, nil
}

// normalize widens the Go values OpenFeature contexts commonly hold that structpb.NewValue rejects.
func normalize(v any) any {
	switch v := v.(type) {
	case []any:
		out := make([]any, len(v))
		for i, e := range v {
			out[i] = normalize(e)
		}
		return out
	case map[string]any:
		out := make(map[string]any, len(v))
		for k, e := range v {
			out[k] = normalize(e)
		}
		return out
	case []string:
		out := make([]any, len(v))
		for i, e := range v {
			out[i] = e
		}
		return out
	case time.Time:
		return v.Format(time.RFC3339Nano)
	default:
		return v
	}
}

func (p *LocalResolverProvider) resolveError(flag string, err error) openfeature.ResolutionError {
	var notSupported *MaterializationNotSupportedError
	switch {
	case errors.Is(err, engine.ErrNotReady):
		return openfeature.NewProviderNotReadyResolutionError(err.Error())
	case errors.Is(err, engine.ErrClosed):
		return openfeature.NewProviderFatalResolutionError(err.Error())
	case errors.As(err, &notSupported):
		p.logger.Warn("flag needs materializations, but no MaterializationStore is configured", "flag", flag)
		return openfeature.NewGeneralResolutionError(fmt.Sprintf("flag %q requires materializations; configure a MaterializationStore", flag))
	default:
		p.logger.Error("flag resolve failed", "flag", flag, "error", err)
		return openfeature.NewGeneralResolutionError(err.Error())
	}
}

func resolutionError(code wasmpb.ErrorCode, msg string) openfeature.ResolutionError {
	switch code {
	case wasmpb.ErrorCode_ERROR_CODE_FLAG_NOT_FOUND:
		return openfeature.NewFlagNotFoundResolutionError(msg)
	case wasmpb.ErrorCode_ERROR_CODE_TYPE_MISMATCH:
		return openfeature.NewTypeMismatchResolutionError(msg)
	case wasmpb.ErrorCode_ERROR_CODE_PROVIDER_NOT_READY:
		return openfeature.NewProviderNotReadyResolutionError(msg)
	case wasmpb.ErrorCode_ERROR_CODE_PROVIDER_FATAL:
		return openfeature.NewProviderFatalResolutionError(msg)
	default:
		return openfeature.NewGeneralResolutionError(msg)
	}
}

func reason(r resolverpb.ResolveReason) openfeature.Reason {
	switch r {
	case resolverpb.ResolveReason_RESOLVE_REASON_MATCH:
		return openfeature.TargetingMatchReason
	case resolverpb.ResolveReason_RESOLVE_REASON_NO_SEGMENT_MATCH:
		return openfeature.DefaultReason
	case resolverpb.ResolveReason_RESOLVE_REASON_FLAG_ARCHIVED:
		return openfeature.DisabledReason
	case resolverpb.ResolveReason_RESOLVE_REASON_TARGETING_KEY_ERROR,
		resolverpb.ResolveReason_RESOLVE_REASON_ERROR,
		resolverpb.ResolveReason_RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE:
		return openfeature.ErrorReason
	default:
		return openfeature.UnknownReason
	}
}

// cut splits "flag.path.to.field" at the first dot.
func cut(key string) (flag, path string, hasPath bool) {
	for i := 0; i < len(key); i++ {
		if key[i] == '.' {
			return key[:i], key[i+1:], true
		}
	}
	return key, "", false
}

// storeAdapter adapts the public MaterializationStore to the module's flat records: a record with
// a rule is a variant (sticky assignment) read, one without is an inclusion read. Absent results
// mean "no assignment" and "not included".
//
// Writes are fire-and-forget, as in earlier versions: a slow or failing store never fails or
// delays a resolve.
type storeAdapter struct {
	store  MaterializationStore
	logger *slog.Logger
}

func (a storeAdapter) Read(ctx context.Context, requested []*wasmpb.Materialization_Record) ([]*wasmpb.Materialization_Record, error) {
	ops := make([]ReadOp, len(requested))
	for i, r := range requested {
		if r.GetRule() != "" {
			ops[i] = newReadOpVariant(r.GetMaterialization(), r.GetUnit(), r.GetRule())
		} else {
			ops[i] = newReadOpInclusion(r.GetMaterialization(), r.GetUnit())
		}
	}
	results, err := a.store.Read(ctx, ops)
	if err != nil {
		return nil, err
	}
	var records []*wasmpb.Materialization_Record
	for _, res := range results {
		switch v := res.(type) {
		case *ReadResultVariant:
			if v.Variant() != nil && *v.Variant() != "" {
				records = append(records, &wasmpb.Materialization_Record{Unit: v.Unit(), Materialization: v.Materialization(), Rule: v.Rule(), Variant: *v.Variant()})
			}
		case *ReadResultInclusion:
			if v.Included() {
				records = append(records, &wasmpb.Materialization_Record{Unit: v.Unit(), Materialization: v.Materialization()})
			}
		}
	}
	return records, nil
}

func (a storeAdapter) Write(ctx context.Context, assignments []*wasmpb.Materialization_Record) error {
	ops := make([]WriteOp, len(assignments))
	for i, r := range assignments {
		ops[i] = newWriteOpVariant(r.GetMaterialization(), r.GetUnit(), r.GetRule(), r.GetVariant())
	}
	go func() {
		var notSupported *MaterializationNotSupportedError
		if err := a.store.Write(context.WithoutCancel(ctx), ops); err != nil && !errors.As(err, &notSupported) {
			a.logger.Warn("materialization write failed", "error", err)
		}
	}()
	return nil
}
