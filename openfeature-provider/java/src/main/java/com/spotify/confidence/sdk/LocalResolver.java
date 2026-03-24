package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.RegisterResolveRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import java.util.concurrent.CompletionStage;

/** Common interface for the compositional local resolver layers. */
interface LocalResolver {

  /**
   * Sets the resolver state (flag configuration).
   *
   * @param state the serialized resolver state
   * @param accountId the account ID
   * @param sdk the SDK identifier and version
   */
  void setResolverState(byte[] state, String accountId, Sdk sdk);

  /**
   * Resolves flags. The returned stage completes when all resolution (including any store I/O for
   * materializations) has finished.
   *
   * @param request the resolve process request
   * @return a stage that completes with the resolve process response
   */
  CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request);

  /**
   * Applies flags that were previously resolved with apply=false.
   *
   * @param request the apply flags request
   */
  void applyFlags(ApplyFlagsRequest request);

  /**
   * Registers a resolve evaluation with reason and latency for telemetry.
   *
   * @param request the register resolve request containing reason and latency
   */
  void registerResolve(RegisterResolveRequest request);

  /** Flushes all pending logs (resolve + assign). */
  void flushAllLogs();

  /** Flushes pending assignment logs only. */
  void flushAssignLogs();

  /** Closes the resolver and releases resources. */
  void close();

  /** Returns a Prometheus metrics snapshot for this resolver instance. */
  String prometheusSnapshot();
}
