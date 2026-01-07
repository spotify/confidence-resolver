package com.spotify.confidence.sdk;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ConcurrentHashMap;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Reference implementation of {@link MaterializationStore} using in-memory storage.
 *
 * <p><strong>⚠️ For Testing/Example Only:</strong> This implementation is suitable for testing and
 * as a reference but should NOT be used in production because:
 *
 * <ul>
 *   <li>Data is lost on application restart (no persistence)
 *   <li>No TTL management (entries never expire)
 *   <li>Memory grows unbounded
 *   <li>Not suitable for multi-instance deployments
 * </ul>
 *
 * <p><strong>Thread Safety:</strong> This implementation is thread-safe using {@link
 * ConcurrentHashMap} for concurrent access.
 *
 * <p><strong>Storage Structure:</strong>
 *
 * <pre>
 * unit → materialization → MaterializationData
 *   where MaterializationData contains:
 *     - included: boolean (whether unit is in materialized segment)
 *     - variants: Map&lt;rule, variant&gt; (sticky variant assignments)
 * </pre>
 *
 * <p><strong>Production Implementation:</strong> For production use, implement {@link
 * MaterializationStore} with persistent storage like Redis, DynamoDB, etc.
 *
 * @see MaterializationStore
 */
public class InMemoryMaterializationStoreExample implements MaterializationStore {

  private static final Logger logger =
      LoggerFactory.getLogger(InMemoryMaterializationStoreExample.class);

  // Storage structure: unit -> materialization -> MaterializationData
  private final Map<String, Map<String, MaterializationData>> storage = new ConcurrentHashMap<>();

  private static class MaterializationData {
    boolean included;
    final Map<String, String> variants; // rule -> variant

    MaterializationData(boolean included, Map<String, String> variants) {
      this.included = included;
      this.variants = new ConcurrentHashMap<>(variants);
    }

    MaterializationData() {
      this(false, new HashMap<>());
    }
  }

  @Override
  public CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops) {
    final List<ReadResult> results = new ArrayList<>();

    for (ReadOp op : ops) {
      if (op instanceof ReadOp.Inclusion inclusionOp) {
        boolean included = false;
        final Map<String, MaterializationData> unitData = storage.get(inclusionOp.unit());
        if (unitData != null) {
          final MaterializationData data = unitData.get(inclusionOp.materialization());
          if (data != null) {
            included = data.included;
          }
        }
        results.add(inclusionOp.toResult(included));
        logger.debug(
            "Read inclusion for unit: {}, materialization: {}, result: {}",
            inclusionOp.unit(),
            inclusionOp.materialization(),
            included);
      } else if (op instanceof ReadOp.Variant variantOp) {
        Optional<String> variant = Optional.empty();
        final Map<String, MaterializationData> unitData = storage.get(variantOp.unit());
        if (unitData != null) {
          final MaterializationData data = unitData.get(variantOp.materialization());
          if (data != null) {
            variant = Optional.ofNullable(data.variants.get(variantOp.rule()));
          }
        }
        results.add(variantOp.toResult(variant));
        logger.debug(
            "Read variant for unit: {}, materialization: {}, rule: {}, result: {}",
            variantOp.unit(),
            variantOp.materialization(),
            variantOp.rule(),
            variant);
      }
    }

    return CompletableFuture.completedFuture(results);
  }

  @Override
  public CompletionStage<Void> write(Set<? extends WriteOp> ops) {
    for (WriteOp op : ops) {
      if (op instanceof WriteOp.Variant variantOp) {
        storage.compute(
            variantOp.unit(),
            (unit, unitData) -> {
              if (unitData == null) {
                unitData = new HashMap<>();
              }
              unitData.compute(
                  variantOp.materialization(),
                  (materialization, data) -> {
                    if (data == null) {
                      data = new MaterializationData();
                    }
                    data.variants.put(variantOp.rule(), variantOp.variant());
                    data.included = true; // Mark as included when we write a variant
                    return data;
                  });
              return unitData;
            });
        logger.debug(
            "Wrote variant for unit: {}, materialization: {}, rule: {}, variant: {}",
            variantOp.unit(),
            variantOp.materialization(),
            variantOp.rule(),
            variantOp.variant());
      }
    }

    return CompletableFuture.completedFuture(null);
  }

  /**
   * Clears all stored materialization data from memory.
   *
   * <p>Call this method during application shutdown or test cleanup to free memory.
   */
  public void close() {
    storage.clear();
    logger.debug("In-memory storage cleared.");
  }
}
