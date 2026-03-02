package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;

/** Common interface for the compositional local resolver layers. */
interface LocalResolver {

  /**
   * Sets the resolver state (flag configuration).
   *
   * @param state the serialized resolver state
   * @param accountId the account ID
   */
  void setResolverState(byte[] state, String accountId);

  /**
   * Resolves flags synchronously.
   *
   * @param request the resolve process request
   * @return the resolve process response
   */
  ResolveProcessResponse resolveProcess(ResolveProcessRequest request);

  /**
   * Applies flags that were previously resolved with apply=false.
   *
   * @param request the apply flags request
   */
  void applyFlags(ApplyFlagsRequest request);

  /** Flushes all pending logs (resolve + assign). */
  void flushAllLogs();

  /** Flushes pending assignment logs only. */
  void flushAssignLogs();

  /** Closes the resolver and releases resources. */
  void close();
}
