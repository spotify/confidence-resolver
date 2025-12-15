package com.spotify.confidence;

import static com.spotify.confidence.GrpcUtil.createConfidenceChannel;

import com.google.common.annotations.VisibleForTesting;
import com.spotify.confidence.flags.resolver.v1.*;
import io.grpc.*;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * A {@link MaterializationStore} implementation that stores materialization data remotely via gRPC
 * to the Confidence service.
 *
 * <p>This implementation is useful when you want the Confidence service to manage materialization
 * storage server-side rather than maintaining local state. The Confidence service automatically
 * handles TTL management (90-day default) for stored materializations.
 *
 * <p><strong>Usage Example:</strong>
 *
 * <pre>{@code
 * String clientSecret = "your-application-client-secret";
 * MaterializationStore store = new RemoteMaterializationStore(clientSecret);
 *
 * // Use with OpenFeatureLocalResolveProvider
 * OpenFeatureLocalResolveProvider provider =
 *     new OpenFeatureLocalResolveProvider(config, clientSecret, store);
 * }</pre>
 *
 * <p><strong>Configuration:</strong> Timeouts can be configured via environment variables:
 *
 * <ul>
 *   <li>{@code CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS} - timeout for read operations
 *       (default: 2 seconds)
 *   <li>{@code CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS} - timeout for write operations
 *       (default: 5 seconds)
 * </ul>
 *
 * <p><strong>Thread Safety:</strong> This implementation is thread-safe and can handle concurrent
 * requests.
 *
 * @see MaterializationStore
 * @see UnsupportedMaterializationStore
 */
public class RemoteMaterializationStore implements MaterializationStore {
  private static final Logger logger = LoggerFactory.getLogger(RemoteMaterializationStore.class);
  private static final Duration DEFAULT_READ_TIMEOUT = Duration.ofSeconds(2);
  private static final Duration DEFAULT_WRITE_TIMEOUT = Duration.ofSeconds(5);

  private final InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub stub;
  private final Duration readTimeout;
  private final Duration writeTimeout;
  private final ExecutorService executor;
  private final boolean shouldShutdownExecutor;
  private ManagedChannel channel;

  /**
   * Creates a new RemoteMaterializationStore with the given client secret.
   *
   * <p>This constructor uses the default channel factory to create a gRPC connection to the
   * Confidence service.
   *
   * @param clientSecret the client secret for authentication with the Confidence service
   */
  public RemoteMaterializationStore(String clientSecret) {
    this(clientSecret, new DefaultChannelFactory());
  }

  /**
   * Creates a new RemoteMaterializationStore with a custom channel factory.
   *
   * <p>This constructor is useful for testing or when you need custom gRPC channel configuration.
   *
   * @param clientSecret the client secret for authentication with the Confidence service
   * @param channelFactory the factory to use for creating gRPC channels
   */
  public RemoteMaterializationStore(String clientSecret, ChannelFactory channelFactory) {
    this.stub = createAuthStub(channelFactory, clientSecret);
    this.readTimeout = getReadTimeout();
    this.writeTimeout = getWriteTimeout();
    this.executor = Executors.newCachedThreadPool();
    this.shouldShutdownExecutor = true;
  }

  @VisibleForTesting
  RemoteMaterializationStore(
      InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub stub,
      Duration readTimeout,
      Duration writeTimeout,
      ExecutorService executor) {
    this.stub = stub;
    this.readTimeout = readTimeout;
    this.writeTimeout = writeTimeout;
    this.executor = executor;
    this.shouldShutdownExecutor = false;
  }

  private InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub createAuthStub(
      ChannelFactory channelFactory, String clientSecret) {
    this.channel = createConfidenceChannel(channelFactory);
    return InternalFlagLoggerServiceGrpc.newBlockingStub(channel)
        .withInterceptors(
            new ClientInterceptor() {
              @Override
              public <ReqT, RespT> ClientCall<ReqT, RespT> interceptCall(
                  MethodDescriptor<ReqT, RespT> method, CallOptions callOptions, Channel next) {
                return new ForwardingClientCall.SimpleForwardingClientCall<ReqT, RespT>(
                    next.newCall(method, callOptions)) {
                  @Override
                  public void start(Listener<RespT> responseListener, Metadata headers) {
                    headers.put(
                        Metadata.Key.of("authorization", Metadata.ASCII_STRING_MARSHALLER),
                        "ClientSecret " + clientSecret);
                    super.start(responseListener, headers);
                  }
                };
              }
            });
  }

  private static Duration getReadTimeout() {
    String envVal = System.getenv("CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS");
    if (envVal != null && !envVal.isEmpty()) {
      try {
        return Duration.ofSeconds(Long.parseLong(envVal));
      } catch (NumberFormatException e) {
        logger.warn(
            "Invalid CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS value: {}, using default",
            envVal);
      }
    }
    return DEFAULT_READ_TIMEOUT;
  }

  private static Duration getWriteTimeout() {
    String envVal = System.getenv("CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS");
    if (envVal != null && !envVal.isEmpty()) {
      try {
        return Duration.ofSeconds(Long.parseLong(envVal));
      } catch (NumberFormatException e) {
        logger.warn(
            "Invalid CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS value: {}, using default",
            envVal);
      }
    }
    return DEFAULT_WRITE_TIMEOUT;
  }

  @Override
  public CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops) {
    if (ops == null || ops.isEmpty()) {
      return CompletableFuture.completedFuture(Collections.emptyList());
    }

    return CompletableFuture.supplyAsync(
        () -> {
          try {
            // Convert ReadOps to proto format
            List<com.spotify.confidence.flags.resolver.v1.ReadOp> protoOps =
                new ArrayList<>(ops.size());
            for (ReadOp op : ops) {
              protoOps.add(readOpToProto(op));
            }

            // Build request
            ReadOperationsRequest request =
                ReadOperationsRequest.newBuilder().addAllOps(protoOps).build();

            // Make gRPC call with timeout
            ReadOperationsResult response =
                stub.withDeadlineAfter(readTimeout.toMillis(), TimeUnit.MILLISECONDS)
                    .readMaterializedOperations(request);

            // Convert proto results to Java types
            List<ReadResult> results = new ArrayList<>(response.getResultsCount());
            for (com.spotify.confidence.flags.resolver.v1.ReadResult protoResult :
                response.getResultsList()) {
              results.add(protoToReadResult(protoResult));
            }

            return results;
          } catch (StatusRuntimeException e) {
            logger.error("Failed to read materialized operations", e);
            throw new RuntimeException("Failed to read materialized operations", e);
          }
        },
        executor);
  }

  @Override
  public CompletionStage<Void> write(Set<? extends WriteOp> ops) {
    if (ops == null || ops.isEmpty()) {
      return CompletableFuture.completedFuture(null);
    }

    return CompletableFuture.runAsync(
        () -> {
          try {
            // Convert WriteOps to proto format
            List<VariantData> protoOps = new ArrayList<>(ops.size());
            for (WriteOp op : ops) {
              protoOps.add(writeOpToProto(op));
            }

            // Build request
            WriteOperationsRequest request =
                WriteOperationsRequest.newBuilder().addAllStoreVariantOp(protoOps).build();

            // Make gRPC call with timeout
            stub.withDeadlineAfter(writeTimeout.toMillis(), TimeUnit.MILLISECONDS)
                .writeMaterializedOperations(request);

          } catch (StatusRuntimeException e) {
            logger.error("Failed to write materialized operations", e);
            throw new RuntimeException("Failed to write materialized operations", e);
          }
        },
        executor);
  }

  public void shutdown() {
    this.channel.shutdown();
    if (shouldShutdownExecutor) {
      this.executor.shutdown();
    }
  }

  private com.spotify.confidence.flags.resolver.v1.ReadOp readOpToProto(ReadOp op) {
    com.spotify.confidence.flags.resolver.v1.ReadOp.Builder builder =
        com.spotify.confidence.flags.resolver.v1.ReadOp.newBuilder();

    if (op instanceof ReadOp.Variant variant) {
      builder.setVariantReadOp(
          VariantReadOp.newBuilder()
              .setUnit(variant.unit())
              .setMaterialization(variant.materialization())
              .setRule(variant.rule())
              .build());
    } else if (op instanceof ReadOp.Inclusion inclusion) {
      builder.setInclusionReadOp(
          InclusionReadOp.newBuilder()
              .setUnit(inclusion.unit())
              .setMaterialization(inclusion.materialization())
              .build());
    } else {
      throw new IllegalArgumentException("Unknown read op type: " + op.getClass().getName());
    }

    return builder.build();
  }

  private ReadResult protoToReadResult(com.spotify.confidence.flags.resolver.v1.ReadResult proto) {
    if (proto.hasVariantResult()) {
      VariantData variantData = proto.getVariantResult();
      Optional<String> variant =
          variantData.getVariant() != null && !variantData.getVariant().isEmpty()
              ? Optional.of(variantData.getVariant())
              : Optional.empty();
      return new ReadResult.Variant(
          variantData.getMaterialization(), variantData.getUnit(), variantData.getRule(), variant);
    } else if (proto.hasInclusionResult()) {
      InclusionData inclusionData = proto.getInclusionResult();
      return new ReadResult.Inclusion(
          inclusionData.getMaterialization(),
          inclusionData.getUnit(),
          inclusionData.getIsIncluded());
    } else {
      throw new IllegalArgumentException("Unknown read result type");
    }
  }

  private VariantData writeOpToProto(WriteOp op) {
    if (op instanceof WriteOp.Variant variant) {
      return VariantData.newBuilder()
          .setUnit(variant.unit())
          .setMaterialization(variant.materialization())
          .setRule(variant.rule())
          .setVariant(variant.variant())
          .build();
    } else {
      throw new IllegalArgumentException("Unknown write op type: " + op.getClass().getName());
    }
  }
}
