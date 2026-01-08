package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveWithStickyRequest;
import java.util.concurrent.CompletionStage;

/** Common interface for WASM-based flag resolver implementations. */
interface ResolverApi {

  /**
   * Resolves flags with sticky assignment support.
   *
   * @param request The resolve request with sticky context
   * @return A future containing the resolve response
   */
  CompletionStage<ResolveFlagsResponse> resolveWithSticky(ResolveWithStickyRequest request);

  void init(byte[] state, String accountId);

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
