package com.spotify.confidence.sdk;

import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.TimeUnit;

/**
 * Default implementation of ChannelFactory that creates standard gRPC channels with security
 * settings based on environment variables.
 *
 * <p>This factory:
 *
 * <ul>
 *   <li>Uses TLS by default, unless CONFIDENCE_GRPC_PLAINTEXT=true
 *   <li>Adds a default deadline interceptor (1 minute timeout)
 *   <li>Configures retry for transient failures (UNAVAILABLE) via gRPC service config
 *   <li>Applies any additional interceptors passed via defaultInterceptors
 * </ul>
 */
public class DefaultChannelFactory implements ChannelFactory {

  static final Map<String, Object> RETRY_SERVICE_CONFIG =
      Map.of(
          "methodConfig",
          List.of(
              Map.of(
                  "name",
                  List.of(
                      Map.of("service", "confidence.flags.resolver.v1.InternalFlagLoggerService")),
                  "retryPolicy",
                  Map.of(
                      "maxAttempts",
                      3.0,
                      "initialBackoff",
                      "0.5s",
                      "maxBackoff",
                      "5s",
                      "backoffMultiplier",
                      2.0,
                      "retryableStatusCodes",
                      List.of("UNAVAILABLE")))));

  @Override
  public ManagedChannel create(String target, List<ClientInterceptor> defaultInterceptors) {
    final boolean useGrpcPlaintext =
        Optional.ofNullable(System.getenv("CONFIDENCE_GRPC_PLAINTEXT"))
            .map(Boolean::parseBoolean)
            .orElse(false);

    ManagedChannelBuilder<?> builder =
        ManagedChannelBuilder.forTarget(target)
            .keepAliveTime(5, TimeUnit.MINUTES)
            .keepAliveTimeout(20, TimeUnit.SECONDS)
            .idleTimeout(30, TimeUnit.MINUTES)
            .defaultServiceConfig(RETRY_SERVICE_CONFIG)
            .enableRetry();

    if (useGrpcPlaintext) {
      builder = builder.usePlaintext();
    }

    // Combine default interceptors with the deadline interceptor
    List<ClientInterceptor> allInterceptors = new ArrayList<>(defaultInterceptors);
    allInterceptors.add(new DefaultDeadlineClientInterceptor(Duration.ofMinutes(1)));

    return builder.intercept(allInterceptors.toArray(new ClientInterceptor[0])).build();
  }
}
