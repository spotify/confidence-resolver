package confidence

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	et "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/event_tracking"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/events"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
)

// confidenceDomain is the Confidence edge gRPC endpoint used for the events
// service and the remote materialization store.
const confidenceDomain = "edge-grpc.spotify.com"

// logForwarder is the delivery sink the provider hands batches to.
type logForwarder interface {
	Enqueue(body []byte, token string)
	Shutdown(deadline time.Duration)
}

// ProviderConfig configures a provider hosting a compiled client module.
//
// Three pieces of information are required and nothing else: the client
// secret identifying the credential, the encryption key paired with it, and
// (optionally) the module base URL. No account identifier is configured: it
// is not present in the artifact path, not present in the module, and not
// needed to send logs.
type ProviderConfig struct {
	ClientSecret string
	// EncryptionKey is required: the hex-encoded AES-256 key that decrypts
	// the credential's module wrapper from the CDN.
	EncryptionKey string
	// ModuleBaseURL overrides the module CDN; defaults to the production
	// host. Intended for testing.
	ModuleBaseURL string
	// IngestionBaseURL overrides the flag-log ingestion API base URL;
	// defaults to the regionless host (the regional eu/us hosts can be set
	// explicitly). Intended for testing.
	IngestionBaseURL string
	Logger           *slog.Logger
	// TransportHooks allows advanced customization of gRPC and HTTP
	// networking (optional; defaults to DefaultTransportHooks).
	TransportHooks TransportHooks
	// MaterializationStore provides persistent storage for materialization
	// data (optional). Without one, resolves that need materializations
	// report MATERIALIZATION_NOT_SUPPORTED per flag.
	MaterializationStore MaterializationStore
	// UseRemoteMaterializationStore looks materializations up through the
	// Confidence remote service. Requires that MaterializationStore is nil.
	UseRemoteMaterializationStore bool
	// ModulePollInterval is how often a fresh module is polled for
	// (optional; defaults to 30 seconds). The CDN caches briefly, so a
	// published change becomes visible within about a minute.
	ModulePollInterval time.Duration
	// LogFlushInterval is how often buffered logs are flushed out of the
	// module (optional; defaults to 5 seconds) so low-volume credentials
	// stay observable.
	LogFlushInterval time.Duration
	// InitializationTimeout is the total startup retry budget (optional;
	// defaults to 30 seconds). The provider keeps retrying in the
	// background when it expires.
	InitializationTimeout time.Duration
	// UseWasmInterpreter runs the WASM engines (the module host and the
	// event engine) on wazero's interpreter instead of its compiler. See the
	// README's interpreter-mode section for the linux/arm64 signal-handling
	// trade-offs.
	UseWasmInterpreter bool
	// InstancePoolSize bounds how many module instances serve concurrent
	// evaluations (one instance per in-flight call, over the shared flag
	// data). 0 defaults to GOMAXPROCS. Each instance costs a few MB of
	// resident memory, so embedding many providers per process may want a
	// smaller cap; throughput saturates well below GOMAXPROCS on most
	// workloads.
	InstancePoolSize int
	// DisableApplyDedup turns off apply-event deduplication, so every apply
	// is logged even when an identical assignment was just logged. Dedup is
	// on by default; use this only if you need the unfiltered apply stream.
	DisableApplyDedup bool
	// DisableExposureCollection disables exposure/assignment collection for
	// all OpenFeature evaluations through this provider. Use only for
	// exceptional no-exposure modes; resolve logs and telemetry are still
	// sent.
	DisableExposureCollection bool
}

// ProviderTestConfig configures a provider against test doubles: the module
// is provided directly instead of being fetched, and log delivery is
// observable through a sink.
type ProviderTestConfig struct {
	// ClientSecret the module under test accepts.
	ClientSecret string
	// Module is the compiled client module to host.
	Module []byte
	// LogToken the wrapper would carry (captured with each batch).
	LogToken string
	// LogSink, when set, receives every batch with its token instead of
	// HTTP delivery (optional).
	LogSink func(body []byte, token string)
	// IngestionBaseURL for log delivery when no LogSink is set (optional).
	IngestionBaseURL          string
	Logger                    *slog.Logger
	MaterializationStore      MaterializationStore
	ModulePollInterval        time.Duration
	LogFlushInterval          time.Duration
	InitializationTimeout     time.Duration
	DisableApplyDedup         bool
	DisableExposureCollection bool
	// Seed pins the module's per-instance random seed, making variant
	// bucketing deterministic (optional).
	Seed *int64
}

// NewProvider creates a provider hosting the credential's compiled client
// module. The provider is NOT_READY until Init loads the first module.
func NewProvider(ctx context.Context, config ProviderConfig) (*LocalResolverProvider, error) {
	if err := validateEncryptionKey(config.EncryptionKey); err != nil {
		return nil, err
	}
	if config.ClientSecret == "" {
		return nil, fmt.Errorf("ClientSecret is required")
	}
	if config.UseRemoteMaterializationStore && config.MaterializationStore != nil {
		return nil, fmt.Errorf("UseRemoteMaterializationStore requires MaterializationStore to be nil")
	}

	logger := config.Logger
	if logger == nil {
		logger = defaultLogger()
	}

	hooks := config.TransportHooks
	if hooks == nil {
		hooks = DefaultTransportHooks
	}

	// One HTTP transport serves module fetch and log delivery, wrapped by
	// the configured hooks.
	transport := hooks.WrapHTTP(http.DefaultTransport)
	fetchClient := &http.Client{Timeout: 30 * time.Second, Transport: transport}

	fetcher, err := NewModuleFetcher(config.ClientSecret, config.EncryptionKey, config.ModuleBaseURL, logger, fetchClient)
	if err != nil {
		return nil, err
	}
	forwarder := NewLogForwarder(LogForwarderConfig{
		BaseURL:    config.IngestionBaseURL,
		Logger:     logger,
		HTTPClient: &http.Client{Timeout: logForwardHTTPTimeout, Transport: transport},
	})

	provider := newLocalResolverProvider(config.ClientSecret, logger, providerOptions{
		modulePollInterval:        config.ModulePollInterval,
		logFlushInterval:          config.LogFlushInterval,
		initializationTimeout:     config.InitializationTimeout,
		disableApplyDedup:         config.DisableApplyDedup,
		disableExposureCollection: config.DisableExposureCollection,
	})
	provider.fetcher = fetcher
	provider.forwarder = forwarder
	provider.useInterpreter = config.UseWasmInterpreter
	provider.instancePoolSize = config.InstancePoolSize

	if config.UseRemoteMaterializationStore {
		conn, err := grpc.NewClient(confidenceDomain, grpc.WithTransportCredentials(credentials.NewTLS(nil)))
		if err != nil {
			return nil, fmt.Errorf("failed to create materialization channel: %w", err)
		}
		provider.matStore = newRemoteMaterializationStore(resolverv1.NewInternalFlagLoggerServiceClient(conn), config.ClientSecret)
	} else if config.MaterializationStore != nil {
		provider.matStore = config.MaterializationStore
	}

	provider.initEventTracking(et.EventEngineWasm, config.UseWasmInterpreter)
	return provider, nil
}

// newProviderForTest creates a provider hosting the given module directly.
// Log batches go to the sink when one is configured, otherwise to a
// forwarder pointed at the given ingestion URL.
func newProviderForTest(ctx context.Context, config ProviderTestConfig) (*LocalResolverProvider, error) {
	if len(config.Module) == 0 {
		return nil, fmt.Errorf("Module is required")
	}
	logger := config.Logger
	if logger == nil {
		logger = defaultLogger()
	}

	provider := newLocalResolverProvider(config.ClientSecret, logger, providerOptions{
		modulePollInterval:        config.ModulePollInterval,
		logFlushInterval:          config.LogFlushInterval,
		initializationTimeout:     config.InitializationTimeout,
		disableApplyDedup:         config.DisableApplyDedup,
		disableExposureCollection: config.DisableExposureCollection,
	})
	provider.fetcher = &staticModuleFetcher{module: config.Module, token: config.LogToken}

	if config.LogSink != nil {
		provider.forwarder = &sinkForwarder{sink: config.LogSink}
	} else {
		provider.forwarder = NewLogForwarder(LogForwarderConfig{
			BaseURL: config.IngestionBaseURL,
			Logger:  logger,
		})
	}
	if config.MaterializationStore != nil {
		provider.matStore = config.MaterializationStore
	}
	if config.Seed != nil {
		seed := *config.Seed
		provider.seedFn = func() int64 { return seed }
	}
	return provider, nil
}

// providerOptions are the constructor-level knobs shared by NewProvider and
// newProviderForTest.
type providerOptions struct {
	modulePollInterval        time.Duration
	logFlushInterval          time.Duration
	initializationTimeout     time.Duration
	disableApplyDedup         bool
	disableExposureCollection bool
}

func newLocalResolverProvider(clientSecret string, logger *slog.Logger, opts providerOptions) *LocalResolverProvider {
	modulePollInterval := opts.modulePollInterval
	if modulePollInterval <= 0 {
		modulePollInterval = defaultModulePollIntervalSeconds * time.Second
	}
	logFlushInterval := opts.logFlushInterval
	if logFlushInterval <= 0 {
		logFlushInterval = defaultLogFlushInterval
	}
	initializationTimeout := opts.initializationTimeout
	if initializationTimeout <= 0 {
		initializationTimeout = defaultInitializationTimeout
	}

	return &LocalResolverProvider{
		clientSecret:              clientSecret,
		logger:                    logger,
		modulePollInterval:        modulePollInterval,
		logFlushInterval:          logFlushInterval,
		initializationTimeout:     initializationTimeout,
		initialRetryInterval:      initialModuleRetryInterval,
		lifecycleEvents:           make(chan openfeature.Event, 2),
		applyFilter:               newSeenFilter(opts.disableApplyDedup),
		schemaFilter:              newSeenFilter(false),
		disableExposureCollection: opts.disableExposureCollection,
		clientSDK:                 clientSDKIdentifier(),
	}
}

// clientSDKIdentifier names this provider to the module and telemetry: the
// provider's name and version ride on every resolve request.
func clientSDKIdentifier() *resolvertypes.Sdk {
	return &resolvertypes.Sdk{
		Sdk:     &resolvertypes.Sdk_CustomId{CustomId: "confidence-sdk-go-local"},
		Version: Version,
	}
}

// initEventTracking enables the optional event engine (OpenFeature Track API).
// Failures disable event tracking but never fail provider construction.
func (p *LocalResolverProvider) initEventTracking(eventWasmBytes []byte, useInterpreter bool) {
	if len(eventWasmBytes) == 0 {
		return
	}
	eventTracker, err := et.NewEventTracker(eventWasmBytes, useInterpreter)
	if err != nil {
		p.logger.Error("Failed to initialize event tracker, event tracking disabled", "error", err)
		return
	}
	p.eventTracker = eventTracker
	conn, err := grpc.NewClient(
		eventsGrpcTarget,
		grpc.WithTransportCredentials(credentials.NewTLS(nil)),
		grpc.WithDefaultServiceConfig(eventsRetryServiceConfig),
	)
	if err != nil {
		p.logger.Error("Failed to create events service channel, event publishing disabled", "error", err)
		if err := eventTracker.Close(); err != nil {
			p.logger.Warn("Failed to close event tracker", "error", err)
		}
		return
	}
	p.eventsConn = conn
	p.eventsClient = events.NewEventsServiceClient(conn)
	p.logger.Info("Event tracking enabled")
}

// staticModuleFetcher serves one fixed artifact, so Init installs it
// immediately. Replace swaps the artifact to simulate a refresh.
type staticModuleFetcher struct {
	mu     sync.Mutex
	module []byte
	token  string
}

func (f *staticModuleFetcher) Fetch(ctx context.Context) (*FetchedModule, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.module) == 0 {
		return nil, &moduleNotPublishedError{fmt.Errorf("no module installed")}
	}
	return &FetchedModule{Module: f.module, LogToken: f.token}, nil
}

// Replace swaps the artifact the static fetcher serves (and its token).
func (f *staticModuleFetcher) Replace(module []byte, token string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.module = module
	f.token = token
}

// sinkForwarder routes batches straight to a test sink.
type sinkForwarder struct {
	sink func(body []byte, token string)
}

func (f *sinkForwarder) Enqueue(body []byte, token string) {
	f.sink(body, token)
}

func (f *sinkForwarder) Shutdown(deadline time.Duration) {}
