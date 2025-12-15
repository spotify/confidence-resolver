package confidence

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"os"

	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
)

const confidenceDomain = "edge-grpc.spotify.com"

type ProviderConfig struct {
	ClientSecret                  string
	Logger                        *slog.Logger
	TransportHooks                TransportHooks       // Optional: defaults to DefaultTransportHooks
	MaterializationStore          MaterializationStore // Optional
	UseRemoteMaterializationStore bool                 // set to true to use a Remote lookup for materializations. Requires that MaterializationStore is nil.
}

type ProviderTestConfig struct {
	StateProvider        StateProvider
	FlagLogger           FlagLogger
	ClientSecret         string
	Logger               *slog.Logger
	MaterializationStore MaterializationStore // Optional
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
	stateProvider := NewFlagsAdminStateFetcherWithTransport(config.ClientSecret, logger, transport)
	flagLogger := fl.NewGrpcWasmFlagLogger(flagLoggerService, config.ClientSecret, logger)
	materializationStore := config.MaterializationStore
	if materializationStore == nil {
		materializationStore = newUnsupportedMaterializationStore()
	}
	if config.UseRemoteMaterializationStore {
		materializationStore = newRemoteMaterializationStore(resolverv1.NewInternalFlagLoggerServiceClient(conn), config.ClientSecret)
	}

	resolverSupplierWithMaterialization := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, materializationStore)
	provider := NewLocalResolverProvider(resolverSupplierWithMaterialization, stateProvider, flagLogger, config.ClientSecret, logger)
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
	resolverSupplierWithMaterialization := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, materializationStore)
	provider := NewLocalResolverProvider(resolverSupplierWithMaterialization, config.StateProvider, config.FlagLogger, config.ClientSecret, logger)

	return provider, nil
}
