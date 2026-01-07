package com.spotify.confidence.sdk;

import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * Default implementation of ChannelFactory that creates standard gRPC channels with security
 * settings based on environment variables.
 *
 * <p>This factory:
 *
 * <ul>
 *   <li>Uses TLS by default, unless CONFIDENCE_GRPC_PLAINTEXT=true
 *   <li>Adds a default deadline interceptor (1 minute timeout)
 *   <li>Applies any additional interceptors passed via defaultInterceptors
 * </ul>
 */
public class DefaultChannelFactory implements ChannelFactory {
  @Override
  public ManagedChannel create(String target, List<ClientInterceptor> defaultInterceptors) {
    final boolean useGrpcPlaintext =
        Optional.ofNullable(System.getenv("CONFIDENCE_GRPC_PLAINTEXT"))
            .map(Boolean::parseBoolean)
            .orElse(false);

    ManagedChannelBuilder<?> builder = ManagedChannelBuilder.forTarget(target);
    if (useGrpcPlaintext) {
      builder = builder.usePlaintext();
    }

    // Combine default interceptors with the deadline interceptor
    List<ClientInterceptor> allInterceptors = new ArrayList<>(defaultInterceptors);
    allInterceptors.add(new DefaultDeadlineClientInterceptor(Duration.ofMinutes(1)));

    return builder.intercept(allInterceptors.toArray(new ClientInterceptor[0])).build();
  }
}
