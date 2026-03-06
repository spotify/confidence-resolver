package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.MaterializationRecord;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.stream.Collectors;
import java.util.stream.Stream;

/**
 * Materialization layer: wraps a {@link LocalResolver} and handles the suspend/resume cycle for
 * deferred materializations. Converts {@code WithoutMaterializations} requests to {@code
 * DeferredMaterializations}, reads/writes materializations from/to the {@link
 * MaterializationStore}, and resumes suspended resolves.
 *
 * <p>All materialization store I/O is composed via {@link CompletionStage} chains so that callers
 * can choose whether to block or remain fully async.
 *
 * <p>Ported from Go's {@code materializationSupportedResolver}.
 */
class MaterializingResolver implements LocalResolver {
  private final LocalResolver delegate;
  private final MaterializationStore store;

  MaterializingResolver(LocalResolver delegate, MaterializationStore store) {
    this.delegate = delegate;
    this.store = store;
  }

  @Override
  public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
    // Convert WithoutMaterializations to DeferredMaterializations so the inner resolver
    // can suspend when materializations are needed — this wrapper handles the suspend/resume.
    ResolveProcessRequest innerRequest = request;
    if (request.hasWithoutMaterializations()) {
      innerRequest =
          ResolveProcessRequest.newBuilder()
              .setDeferredMaterializations(request.getWithoutMaterializations())
              .build();
    }
    return delegate.resolveProcess(innerRequest).thenCompose(this::handleResponse);
  }

  private CompletionStage<ResolveProcessResponse> handleResponse(ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        storeWritesIfPresent(response.getResolved());
        return CompletableFuture.completedFuture(response);
      }
      case SUSPENDED -> {
        final var suspended = response.getSuspended();
        return readMaterializations(suspended.getMaterializationsToReadList())
            .thenCompose(
                materializations -> {
                  final var resumeRequest = buildResumeRequest(suspended, materializations);
                  return delegate.resolveProcess(resumeRequest);
                })
            .thenCompose(this::handleResumeResponse);
      }
      default ->
          throw new RuntimeException("Unexpected resolve result: " + response.getResultCase());
    }
  }

  private CompletionStage<ResolveProcessResponse> handleResumeResponse(
      ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        storeWritesIfPresent(response.getResolved());
        return CompletableFuture.completedFuture(response);
      }
      case SUSPENDED -> throw new RuntimeException("Unexpected second suspend after resume");
      default ->
          throw new RuntimeException(
              "Unexpected resolve result after resume: " + response.getResultCase());
    }
  }

  private void storeWritesIfPresent(ResolveProcessResponse.Resolved resolved) {
    final var records = resolved.getMaterializationsToWriteList();
    if (records.isEmpty()) {
      return;
    }
    final Set<MaterializationStore.WriteOp> writeOps =
        records.stream()
            .map(
                r ->
                    new MaterializationStore.WriteOp.Variant(
                        r.getMaterialization(), r.getUnit(), r.getRule(), r.getVariant()))
            .collect(Collectors.toSet());
    store.write(writeOps);
  }

  /**
   * Reads materializations from the store and converts results to {@link MaterializationRecord}s.
   * Records with no prior assignment or not-included inclusions are omitted (absence signals no
   * data).
   */
  private CompletionStage<List<MaterializationRecord>> readMaterializations(
      List<MaterializationRecord> toRead) {
    final List<? extends MaterializationStore.ReadOp> readOps =
        toRead.stream()
            .map(
                record -> {
                  if (!record.getRule().isEmpty()) {
                    return new MaterializationStore.ReadOp.Variant(
                        record.getMaterialization(), record.getUnit(), record.getRule());
                  } else {
                    return new MaterializationStore.ReadOp.Inclusion(
                        record.getMaterialization(), record.getUnit());
                  }
                })
            .toList();

    return store
        .read(readOps)
        .thenApply(
            results ->
                results.stream()
                    .flatMap(
                        rr -> {
                          if (rr instanceof MaterializationStore.ReadResult.Variant variant) {
                            if (variant.variant().isPresent()) {
                              return Stream.of(
                                  MaterializationRecord.newBuilder()
                                      .setUnit(variant.unit())
                                      .setMaterialization(variant.materialization())
                                      .setRule(variant.rule())
                                      .setVariant(variant.variant().get())
                                      .build());
                            }
                          }
                          if (rr instanceof MaterializationStore.ReadResult.Inclusion inclusion) {
                            if (inclusion.included()) {
                              return Stream.of(
                                  MaterializationRecord.newBuilder()
                                      .setUnit(inclusion.unit())
                                      .setMaterialization(inclusion.materialization())
                                      .build());
                            }
                          }
                          return Stream.empty();
                        })
                    .toList());
  }

  private ResolveProcessRequest buildResumeRequest(
      ResolveProcessResponse.Suspended suspended, List<MaterializationRecord> materializations) {
    return ResolveProcessRequest.newBuilder()
        .setResume(
            ResolveProcessRequest.Resume.newBuilder()
                .addAllMaterializations(materializations)
                .setState(suspended.getState()))
        .build();
  }

  // --- Pass-through to delegate ---

  @Override
  public void setResolverState(byte[] state, String accountId) {
    delegate.setResolverState(state, accountId);
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    delegate.applyFlags(request);
  }

  @Override
  public void flushAllLogs() {
    delegate.flushAllLogs();
  }

  @Override
  public void flushAssignLogs() {
    delegate.flushAssignLogs();
  }

  @Override
  public void close() {
    delegate.close();
  }
}
