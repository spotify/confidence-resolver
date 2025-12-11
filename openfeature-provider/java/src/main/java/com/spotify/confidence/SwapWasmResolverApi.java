package com.spotify.confidence;

import com.spotify.confidence.flags.resolver.v1.*;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicReference;
import java.util.stream.Collectors;

class SwapWasmResolverApi implements ResolverApi {
  private static final int MAX_CLOSED_RETRIES = 10;
  private static final int MAX_MATERIALIZATION_RETRIES = 3;

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

    // Create initial instance
    final WasmResolveApi initialInstance = new WasmResolveApi(flagLogger);
    initialInstance.setResolverState(initialState, accountId);
    this.wasmResolverApiRef.set(initialInstance);
  }

  @Override
  public void init(byte[] state, String accountId) {
    updateStateAndFlushLogs(state, accountId);
  }

  @Override
  public void updateStateAndFlushLogs(byte[] state, String accountId) {
    // Create new instance with updated state
    final WasmResolveApi newInstance = new WasmResolveApi(flagLogger);
    newInstance.setResolverState(state, accountId);

    // Get current instance before switching
    final WasmResolveApi oldInstance = wasmResolverApiRef.getAndSet(newInstance);
    if (oldInstance != null) {
      oldInstance.close();
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
  public CompletionStage<ResolveFlagsResponse> resolveWithSticky(ResolveWithStickyRequest request) {
    return resolveWithStickyInternal(request, 0, 0);
  }

  private CompletionStage<ResolveFlagsResponse> resolveWithStickyInternal(
      ResolveWithStickyRequest request, int closedRetries, int materializationRetries) {
    final var instance = wasmResolverApiRef.get();
    final ResolveWithStickyResponse response;
    try {
      response = instance.resolveWithSticky(request);
    } catch (IsClosedException e) {
      if (closedRetries >= MAX_CLOSED_RETRIES) {
        throw new RuntimeException(
            "Max retries exceeded for IsClosedException: " + MAX_CLOSED_RETRIES, e);
      }
      return resolveWithStickyInternal(request, closedRetries + 1, materializationRetries);
    }

    switch (response.getResolveResultCase()) {
      case SUCCESS -> {
        final var success = response.getSuccess();
        if (!success.getMaterializationUpdatesList().isEmpty()) {
          storeUpdates(success.getMaterializationUpdatesList());
        }
        return CompletableFuture.completedFuture(success.getResponse());
      }
      case READ_OPS_REQUEST -> {
        if (materializationRetries >= MAX_MATERIALIZATION_RETRIES) {
          throw new RuntimeException(
              "Max retries exceeded for missing materializations: " + MAX_MATERIALIZATION_RETRIES);
        }
        return handleMissingMaterializations(request, response.getReadOpsRequest().getOpsList())
            .thenCompose(
                req -> resolveWithStickyInternal(req, closedRetries, materializationRetries + 1));
      }
      case RESOLVERESULT_NOT_SET ->
          throw new RuntimeException("Invalid response: resolve result not set");
      default ->
          throw new RuntimeException("Unhandled response case: " + response.getResolveResultCase());
    }
  }

  private CompletionStage<Void> storeUpdates(List<VariantData> updates) {
    final Set<MaterializationStore.WriteOp> writeOps =
        updates.stream()
            .map(
                u ->
                    new MaterializationStore.WriteOp.Variant(
                        u.getMaterialization(), u.getUnit(), u.getRule(), u.getVariant()))
            .collect(Collectors.toSet());

    return materializationStore.write(writeOps);
  }

  private CompletionStage<ResolveWithStickyRequest> handleMissingMaterializations(
      ResolveWithStickyRequest request, List<ReadOp> readOpList) {

    final List<? extends MaterializationStore.ReadOp> readOps =
        readOpList.stream()
            .map(
                ro -> {
                  switch (ro.getOpCase()) {
                    case VARIANT_READ_OP -> {
                      final var variantReadOp = ro.getVariantReadOp();
                      return new MaterializationStore.ReadOp.Variant(
                          variantReadOp.getMaterialization(),
                          variantReadOp.getUnit(),
                          variantReadOp.getRule());
                    }
                    case INCLUSION_READ_OP -> {
                      final var inclusionReadOp = ro.getInclusionReadOp();
                      return new MaterializationStore.ReadOp.Inclusion(
                          inclusionReadOp.getMaterialization(), inclusionReadOp.getUnit());
                    }
                    default ->
                        throw new RuntimeException("Unhandled read op case: " + ro.getOpCase());
                  }
                })
            .toList();

    return materializationStore
        .read(readOps)
        .thenApply(
            results -> {
              final ResolveWithStickyRequest.Builder requestBuilder = request.toBuilder();
              results.forEach(
                  rr -> {
                    final var builder = ReadResult.newBuilder();
                    if (rr instanceof MaterializationStore.ReadResult.Variant variant) {
                      builder.setVariantResult(
                          VariantData.newBuilder()
                              .setMaterialization(variant.materialization())
                              .setUnit(variant.unit())
                              .setRule(variant.rule())
                              .setVariant(variant.variant().orElse("")));
                    }
                    if (rr instanceof MaterializationStore.ReadResult.Inclusion inclusion) {
                      builder.setInclusionResult(
                          InclusionData.newBuilder()
                              .setMaterialization(inclusion.materialization())
                              .setUnit(inclusion.unit())
                              .setIsIncluded(inclusion.included()));
                    }
                    requestBuilder.addMaterializations(builder.build());
                  });
              return requestBuilder.build();
            });
  }
}
