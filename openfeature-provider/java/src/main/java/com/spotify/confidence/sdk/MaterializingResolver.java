package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.MaterializationRecord;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import java.util.List;
import java.util.Set;
import java.util.stream.Collectors;
import java.util.stream.Stream;

/**
 * Materialization layer: wraps a {@link LocalResolver} and handles the suspend/resume cycle for
 * deferred materializations. Converts {@code WithoutMaterializations} requests to {@code
 * DeferredMaterializations}, reads/writes materializations from/to the {@link
 * MaterializationStore}, and resumes suspended resolves.
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
  public ResolveProcessResponse resolveProcess(ResolveProcessRequest request) {
    // Convert WithoutMaterializations to DeferredMaterializations so the inner resolver
    // can suspend when materializations are needed — this wrapper handles the suspend/resume.
    ResolveProcessRequest innerRequest = request;
    if (request.hasWithoutMaterializations()) {
      innerRequest =
          ResolveProcessRequest.newBuilder()
              .setDeferredMaterializations(request.getWithoutMaterializations())
              .build();
    }
    final ResolveProcessResponse response = delegate.resolveProcess(innerRequest);
    return handleResponse(response);
  }

  private ResolveProcessResponse handleResponse(ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        storeWritesIfPresent(response.getResolved());
        return response;
      }
      case SUSPENDED -> {
        final var suspended = response.getSuspended();
        final var resumeRequest = buildResumeRequest(suspended);
        final var resumeResponse = delegate.resolveProcess(resumeRequest);
        return handleResumeResponse(resumeResponse);
      }
      default ->
          throw new RuntimeException("Unexpected resolve result: " + response.getResultCase());
    }
  }

  private ResolveProcessResponse handleResumeResponse(ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        storeWritesIfPresent(response.getResolved());
        return response;
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
    store.write(writeOps).toCompletableFuture().join();
  }

  private ResolveProcessRequest buildResumeRequest(ResolveProcessResponse.Suspended suspended) {
    final List<MaterializationRecord> materializations =
        readMaterializations(suspended.getMaterializationsToReadList());
    return ResolveProcessRequest.newBuilder()
        .setResume(
            ResolveProcessRequest.Resume.newBuilder()
                .addAllMaterializations(materializations)
                .setState(suspended.getState()))
        .build();
  }

  /**
   * Reads materializations from the store and converts results to {@link MaterializationRecord}s.
   * Records with no prior assignment or not-included inclusions are omitted (absence signals no
   * data).
   */
  private List<MaterializationRecord> readMaterializations(List<MaterializationRecord> toRead) {
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

    final var results = store.read(readOps).toCompletableFuture().join();

    return results.stream()
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
        .toList();
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
