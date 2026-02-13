package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import java.util.concurrent.CompletionStage;

/** Common interface for WASM-based flag resolver implementations. */
interface ResolverApi {

  /**
   * Resolves flags with materialization support (suspend/resume).
   *
   * @param request The resolve process request
   * @return A future containing the resolve response
   */
  CompletionStage<ResolveFlagsResponse> resolveProcess(ResolveProcessRequest request);

  void init(byte[] state, String accountId);

  /** Returns true if the resolver has been initialized with valid state. */
  boolean isInitialized();

  /**
   * Updates the resolver state and flushes any pending logs.
   *
   * @param state The new resolver state
   * @param accountId The account ID
   */
  void updateStateAndFlushLogs(byte[] state, String accountId);

  /** Closes the resolver and releases any resources. */
  void close();
}
