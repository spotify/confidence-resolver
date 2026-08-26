package confidence

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"strconv"
	"time"

	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
)

const confidenceDomain = "edge-grpc.spotify.com"

type ProviderConfig struct {
	ClientSecret                  string
	EncryptionKey                 string // Optional: hex-encoded AES-256 key for decrypting CDN state
	Logger                        *slog.Logger
	TransportHooks                TransportHooks       // Optional: defaults to DefaultTransportHooks
	MaterializationStore          MaterializationStore // Optional
	UseRemoteMaterializationStore bool                 // set to true to use a Remote lookup for materializations. Requires that MaterializationStore is nil.
	StatePollInterval             time.Duration        // Optional: interval for state polling, defaults to 10 seconds
	LogPollInterval               time.Duration        // Optional: interval for log flushing, defaults to 60 seconds
	ResolverPoolSize              int                  // Optional: number of WASM resolver instances in the pool, defaults to 2
	UseWasmInterpreter            bool                 // Optional: wazero interpreter instead of JIT — see provider README; default false
	EnableApplyDedup              *bool                // Optional: apply-event dedup in the WASM resolver; on by default — set to false to disable
	// DisableExposureCollection disables exposure/assignment collection for all
	// OpenFeature evaluations through this provider. Use only for exceptional
	// no-exposure modes; resolve logs and telemetry are still sent.
	DisableExposureCollection bool
}

type ProviderTestConfig struct {
	StateProvider        StateProvider
	FlagLogger           FlagLogger
	ClientSecret         string
	Logger               *slog.Logger
	MaterializationStore MaterializationStore // Optional
	StatePollInterval    time.Duration        // Optional: interval for state polling, defaults to 10 seconds
	LogPollInterval      time.Duration        // Optional: interval for log flushing, defaults to 60 seconds
	ResolverPoolSize     int                  // Optional: number of WASM resolver instances in the pool, defaults to 2
	UseWasmInterpreter   bool                 // Optional: wazero interpreter instead of JIT — see provider README; default false
	// DisableExposureCollection disables exposure/assignment collection for all
	// OpenFeature evaluations through this provider. Use only for exceptional
	// no-exposure modes; resolve logs and telemetry are still sent.
	DisableExposureCollection bool
}

func NewProvider(ctx context.Context, config ProviderConfig) (*LocalResolverProvider, error) {
	if config.ClientSecret == "" {
		return nil, fmt.Errorf("ClientSecret is required")
	}

	logger := config.Logger
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
			Level: slog.LevelInfo,
		}))
	}

	if config.EncryptionKey == "" {
		logger.Warn("No EncryptionKey provided. Falling back to unencrypted state. An encryption key will be required in an upcoming version.")
	}

	// Create gRPC connection for flag logger
	hooks := config.TransportHooks
	if hooks == nil {
		hooks = DefaultTransportHooks
	}

	tlsCreds := credentials.NewTLS(nil)
	baseOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(tlsCreds),
	}

	target, opts := hooks.ModifyGRPCDial(confidenceDomain, baseOpts)
	conn, err := grpc.NewClient(target, opts...)
	if err != nil {
		return nil, fmt.Errorf("failed to create connection: %w", err)
	}

	// Create state provider and flag logger
	flagLoggerService := resolverv1.NewInternalFlagLoggerServiceClient(conn)
	// Build HTTP transport using hooks and pass into state fetcher
	transport := hooks.WrapHTTP(http.DefaultTransport)
	stateProvider := NewFlagsAdminStateFetcherWithEncryption(config.ClientSecret, config.EncryptionKey, logger, transport)
	flagLogger := fl.NewMultiDestinationFlagLogger(
		flagLoggerService,
		config.ClientSecret,
		stateProvider.GetLogDestinations,
		stateProvider.GetAccountID,
		logger,
	)
	materializationStore := config.MaterializationStore
	if materializationStore == nil {
		materializationStore = newUnsupportedMaterializationStore()
	}
	if config.UseRemoteMaterializationStore {
		materializationStore = newRemoteMaterializationStore(resolverv1.NewInternalFlagLoggerServiceClient(conn), config.ClientSecret)
	}

	initLabels := map[string]string{
		"encryption": strconv.FormatBool(config.EncryptionKey != ""),
	}
	resolverSupplier := newLocalResolverSupplier(config.ResolverPoolSize, config.UseWasmInterpreter, initLabels)
	resolverSupplierWithMaterialization := wrapResolverSupplierWithMaterializations(resolverSupplier, materializationStore)
	providerOpts := buildProviderOptions(config.StatePollInterval, config.LogPollInterval, config.EnableApplyDedup, config.DisableExposureCollection)
	provider := NewLocalResolverProvider(resolverSupplierWithMaterialization, stateProvider, flagLogger, config.ClientSecret, logger, providerOpts...)
	return provider, nil
}

// NewProviderForTest creates a provider with mocked StateProvider and FlagLogger for testing
func NewProviderForTest(ctx context.Context, config ProviderTestConfig) (*LocalResolverProvider, error) {
	if config.StateProvider == nil {
		return nil, fmt.Errorf("StateProvider is required")
	}
	if config.FlagLogger == nil {
		return nil, fmt.Errorf("FlagLogger is required")
	}

	logger := config.Logger
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
			Level: slog.LevelInfo,
		}))
	}

	materializationStore := config.MaterializationStore
	if materializationStore == nil {
		materializationStore = newUnsupportedMaterializationStore()
	}
	resolverSupplier := newLocalResolverSupplier(config.ResolverPoolSize, config.UseWasmInterpreter, nil)
	resolverSupplierWithMaterialization := wrapResolverSupplierWithMaterializations(resolverSupplier, materializationStore)
	providerOpts := buildProviderOptions(config.StatePollInterval, config.LogPollInterval, nil, config.DisableExposureCollection)
	provider := NewLocalResolverProvider(resolverSupplierWithMaterialization, config.StateProvider, config.FlagLogger, config.ClientSecret, logger, providerOpts...)

	return provider, nil
}

func newLocalResolverSupplier(poolSize int, useWasmInterpreter bool, initLabels map[string]string) func(context.Context, lr.LogSink) lr.LocalResolver {
	cfg := lr.LocalResolverConfig{
		PoolSize:           poolSize,
		UseWasmInterpreter: useWasmInterpreter,
	}
	return func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return newProviderTelemetryResolver(
			logSink,
			initLabels,
			func(providerLogSink lr.LogSink) lr.LocalResolver {
				return lr.NewLocalResolver(ctx, providerLogSink, cfg)
			},
		)
	}
}

// buildProviderOptions creates options slice from provider config
func buildProviderOptions(statePollInterval, logPollInterval time.Duration, enableApplyDedup *bool, disableExposureCollection bool) []Option {
	var opts []Option
	if statePollInterval > 0 {
		opts = append(opts, WithStatePollInterval(statePollInterval))
	}
	if logPollInterval > 0 {
		opts = append(opts, WithLogPollInterval(logPollInterval))
	}
	if enableApplyDedup == nil || *enableApplyDedup {
		opts = append(opts, WithEnableApplyDedup())
	}
	if disableExposureCollection {
		opts = append(opts, WithDisableExposureCollection())
	}
	return opts
}
