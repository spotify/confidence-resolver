package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.*;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicReference;
import java.util.stream.Collectors;

class SwapWasmResolverApi implements ResolverApi {
  private static final int MAX_CLOSED_RETRIES = 10;

  private final AtomicReference<WasmResolveApi> wasmResolverApiRef = new AtomicReference<>();
  private final MaterializationStore materializationStore;
  private final WasmFlagLogger flagLogger;

  public SwapWasmResolverApi(
      WasmFlagLogger flagLogger,
      byte[] initialState,
      String accountId,
      MaterializationStore materializationStore) {
    this.materializationStore = materializationStore;
    this.flagLogger = flagLogger;

    final WasmResolveApi initialInstance = new WasmResolveApi(flagLogger);
    initialInstance.setResolverState(initialState, accountId);
    this.wasmResolverApiRef.set(initialInstance);
  }

  @Override
  public void init(byte[] state, String accountId) {
    updateStateAndFlushLogs(state, accountId);
  }

  @Override
  public boolean isInitialized() {
    return true; // Always initialized since constructor sets up the instance
  }

  @Override
  public void updateStateAndFlushLogs(byte[] state, String accountId) {
    final WasmResolveApi newInstance = new WasmResolveApi(flagLogger);
    newInstance.setResolverState(state, accountId);

    // Get current instance before switching
    final WasmResolveApi oldInstance = wasmResolverApiRef.getAndSet(newInstance);
    if (oldInstance != null) {
      oldInstance.close();
    }
  }

  @Override
  public void flushAssignLogs() {
    final WasmResolveApi currentInstance = wasmResolverApiRef.get();
    if (currentInstance != null) {
      currentInstance.flushAssignLogs();
    }
  }

  /**
   * Closes the current WasmResolveApi instance, flushing any pending logs. This ensures all
   * buffered log data is sent before shutdown completes.
   */
  @Override
  public void close() {
    final WasmResolveApi currentInstance = wasmResolverApiRef.getAndSet(null);
    if (currentInstance != null) {
      currentInstance.close();
    }
  }

  @Override
  public CompletionStage<ResolveFlagsResponse> resolveProcess(ResolveProcessRequest request) {
    // Convert WithoutMaterializations to DeferredMaterializations so the WASM resolver
    // can suspend when materializations are needed — this middleware handles suspend/resume.
    ResolveProcessRequest innerRequest = request;
    if (request.hasWithoutMaterializations()) {
      innerRequest =
          ResolveProcessRequest.newBuilder()
              .setDeferredMaterializations(request.getWithoutMaterializations())
              .build();
    }
    return callWasm(innerRequest).thenCompose(this::handleResponse);
  }

  private CompletionStage<ResolveFlagsResponse> handleResponse(ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        final var resolved = response.getResolved();
        if (!resolved.getMaterializationsToWriteList().isEmpty()) {
          storeWrites(resolved.getMaterializationsToWriteList());
        }
        return CompletableFuture.completedFuture(resolved.getResponse());
      }
      case SUSPENDED -> {
        final var suspended = response.getSuspended();
        return handleSuspended(suspended)
            .thenCompose(this::callWasm)
            .thenCompose(this::handleResumeResponse);
      }
      case RESULT_NOT_SET -> throw new RuntimeException("Invalid response: resolve result not set");
      default -> throw new RuntimeException("Unhandled response case: " + response.getResultCase());
    }
  }

  private CompletionStage<ResolveFlagsResponse> handleResumeResponse(
      ResolveProcessResponse response) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        final var resolved = response.getResolved();
        if (!resolved.getMaterializationsToWriteList().isEmpty()) {
          storeWrites(resolved.getMaterializationsToWriteList());
        }
        return CompletableFuture.completedFuture(resolved.getResponse());
      }
      case SUSPENDED -> throw new RuntimeException("Unexpected second suspend after resume");
      case RESULT_NOT_SET ->
          throw new RuntimeException("Invalid response after resume: result not set");
      default ->
          throw new RuntimeException(
              "Unhandled response case after resume: " + response.getResultCase());
    }
  }

  private CompletionStage<ResolveProcessResponse> callWasm(ResolveProcessRequest request) {
    return callWasmWithRetry(request, 0);
  }

  private CompletionStage<ResolveProcessResponse> callWasmWithRetry(
      ResolveProcessRequest request, int closedRetries) {
    final var instance = wasmResolverApiRef.get();
    try {
      return CompletableFuture.completedFuture(instance.resolveProcess(request));
    } catch (IsClosedException e) {
      if (closedRetries >= MAX_CLOSED_RETRIES) {
        throw new RuntimeException(
            "Max retries exceeded for IsClosedException: " + MAX_CLOSED_RETRIES, e);
      }
      return callWasmWithRetry(request, closedRetries + 1);
    }
  }

  private CompletionStage<Void> storeWrites(List<MaterializationRecord> records) {
    final Set<MaterializationStore.WriteOp> writeOps =
        records.stream()
            .map(
                r ->
                    new MaterializationStore.WriteOp.Variant(
                        r.getMaterialization(), r.getUnit(), r.getRule(), r.getVariant()))
            .collect(Collectors.toSet());

    return materializationStore.write(writeOps);
  }

  private CompletionStage<ResolveProcessRequest> handleSuspended(
      ResolveProcessResponse.Suspended suspended) {
    final List<MaterializationRecord> toRead = suspended.getMaterializationsToReadList();

    // Convert MaterializationRecords to ReadOps
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

    return materializationStore
        .read(readOps)
        .thenApply(
            results -> {
              // Convert read results to MaterializationRecords for the resume request.
              // "Not included" inclusion results are omitted (absence = not included).
              final var materializations =
                  results.stream()
                      .flatMap(
                          rr -> {
                            if (rr instanceof MaterializationStore.ReadResult.Variant variant) {
                              if (variant.variant().isPresent()) {
                                return java.util.stream.Stream.of(
                                    MaterializationRecord.newBuilder()
                                        .setUnit(variant.unit())
                                        .setMaterialization(variant.materialization())
                                        .setRule(variant.rule())
                                        .setVariant(variant.variant().get())
                                        .build());
                              }
                              // No prior assignment → omit (absence = no sticky assignment)
                            }
                            if (rr instanceof MaterializationStore.ReadResult.Inclusion inclusion) {
                              if (inclusion.included()) {
                                return java.util.stream.Stream.of(
                                    MaterializationRecord.newBuilder()
                                        .setUnit(inclusion.unit())
                                        .setMaterialization(inclusion.materialization())
                                        .build());
                              }
                              // Not included → omit (absence = not included)
                            }
                            return java.util.stream.Stream.empty();
                          })
                      .toList();

              return ResolveProcessRequest.newBuilder()
                  .setResume(
                      ResolveProcessRequest.Resume.newBuilder()
                          .addAllMaterializations(materializations)
                          .setState(suspended.getState()))
                  .build();
            });
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    applyFlagsInternal(request, 0);
  }

  private void applyFlagsInternal(ApplyFlagsRequest request, int closedRetries) {
    final var instance = wasmResolverApiRef.get();
    try {
      instance.applyFlags(request);
    } catch (IsClosedException e) {
      if (closedRetries >= MAX_CLOSED_RETRIES) {
        throw new RuntimeException(
            "Max retries exceeded for IsClosedException: " + MAX_CLOSED_RETRIES, e);
      }
      applyFlagsInternal(request, closedRetries + 1);
    }
  }
}
