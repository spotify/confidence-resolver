package com.spotify.confidence.sdk;

import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import java.util.List;

/**
 * ChannelFactory is an advanced/testing hook allowing callers to customize how gRPC channels are
 * created. The provider will pass the computed target and its default interceptors where
 * applicable.
 *
 * <p>Implementations may modify interceptors, change targets, or replace the channel creation
 * mechanism entirely. Returning a channel with incompatible security/auth can break functionality;
 * use with care.
 *
 * <p>This is particularly useful for:
 *
 * <ul>
 *   <li>Unit testing: inject in-process channels with mock servers
 *   <li>Integration testing: point to local mock gRPC servers
 *   <li>Production customization: custom TLS settings, proxies, connection pooling
 *   <li>Debugging: add custom logging or tracing interceptors
 * </ul>
 *
 * <p><strong>Channel Lifecycle:</strong> Channels created by this factory must be shut down by the
 * caller when they are no longer needed. The factory does not manage channel lifecycles.
 *
 * <p><strong>Note:</strong> The gRPC types ({@code ManagedChannel}, {@code ClientInterceptor}) used
 * in this interface are shaded to {@code com.spotify.confidence.shaded.io.grpc.*}. Implementations
 * must use these shaded types rather than standard gRPC imports.
 */
public interface ChannelFactory {
  /**
   * Creates a gRPC channel with the given target and interceptors.
   *
   * @param target the gRPC target address (e.g., "edge-grpc.spotify.com")
   * @param defaultInterceptors the default interceptors that should be applied
   * @return a configured ManagedChannel
   */
  ManagedChannel create(String target, List<ClientInterceptor> defaultInterceptors);
}
