package com.spotify.confidence.sdk;

import java.util.List;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletionStage;

/**
 * Storage abstraction for materialization data used in flag resolution.
 *
 * <p>Materializations support two key use cases:
 *
 * <ul>
 *   <li><strong>Sticky Assignments:</strong> Maintain consistent variant assignments across
 *       evaluations even when targeting attributes change. This enables pausing intake (stopping
 *       new users from entering an experiment) while keeping existing users in their assigned
 *       variants.
 *   <li><strong>Custom Targeting via Materialized Segments:</strong> Precomputed sets of
 *       identifiers from datasets that should be targeted. Instead of evaluating complex targeting
 *       rules at runtime, materializations allow efficient lookup of whether a unit (user, session,
 *       etc.) is included in a target segment.
 * </ul>
 *
 * <p><strong>Default Behavior:</strong> By default, the provider uses {@link
 * UnsupportedMaterializationStore} which triggers remote resolution via gRPC to the Confidence
 * service. Confidence manages materializations server-side with automatic 90-day TTL management.
 *
 * <p><strong>Custom Implementations:</strong> Optionally implement this interface to store
 * materialization data in your own infrastructure (Redis, database, etc.) to eliminate network
 * calls and improve latency during flag resolution.
 *
 * <p><strong>Thread Safety:</strong> Implementations must be thread-safe as they may be called
 * concurrently from multiple threads resolving flags in parallel.
 *
 * <p><strong>Example Implementation:</strong> See {@code InMemoryMaterializationStoreExample} for a
 * reference implementation using in-memory storage.
 *
 * <p><strong>Key Concepts:</strong>
 *
 * <ul>
 *   <li><strong>Materialization:</strong> An identifier for a materialization context (experiment,
 *       feature flag, or materialized segment)
 *   <li><strong>Unit:</strong> The entity identifier (user ID, session ID, etc.)
 *   <li><strong>Rule:</strong> The targeting rule identifier within a flag
 *   <li><strong>Variant:</strong> The assigned variant name for the unit+rule combination
 * </ul>
 *
 * @see UnsupportedMaterializationStore
 */
public interface MaterializationStore {

  /**
   * Performs a batch read of materialization data.
   *
   * <p>The resolver calls this method to fetch stored materialization data, including sticky
   * assignments and materialized segment memberships.
   *
   * @param ops the list of read operations to perform, never null
   * @return a CompletionStage that completes with the read results
   * @throws MaterializationNotSupportedException if the store doesn't support reads (triggers
   *     fallback to remote gRPC resolution)
   */
  CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops)
      throws MaterializationNotSupportedException;

  /**
   * Performs a batch write of materialization data.
   *
   * <p>The resolver calls this method to persist materialization data after successful flag
   * resolution. This includes storing sticky variant assignments and materialized segment
   * memberships. Implementations should be idempotent.
   *
   * <p><strong>Default Implementation:</strong> Throws {@link
   * MaterializationNotSupportedException}. Override this method if you want to support writing
   * materialization data.
   *
   * @param ops the set of write operations to perform, never null
   * @return a CompletionStage that completes when all writes are finished
   * @throws MaterializationNotSupportedException by default if not overridden
   */
  default CompletionStage<Void> write(Set<? extends WriteOp> ops)
      throws MaterializationNotSupportedException {
    throw new MaterializationNotSupportedException();
  }

  /**
   * Represents a write operation to store materialization data.
   *
   * <p>This sealed interface ensures type-safety and is currently implemented only by {@link
   * Variant}.
   */
  sealed interface WriteOp permits WriteOp.Variant {
    /** Returns the materialization identifier. */
    String materialization();

    /** Returns the unit identifier (user ID, session ID, etc.). */
    String unit();

    /**
     * A variant assignment write operation.
     *
     * <p>Used to store sticky variant assignments, recording which variant a unit (user, session,
     * etc.) should receive for a specific targeting rule.
     *
     * @param materialization the materialization context identifier
     * @param unit the entity identifier (user ID, session ID, etc.)
     * @param rule the targeting rule identifier
     * @param variant the assigned variant name
     */
    record Variant(String materialization, String unit, String rule, String variant)
        implements WriteOp {}
  }

  /**
   * Represents the result of a read operation.
   *
   * <p>This sealed interface ensures type-safety and is implemented by {@link Inclusion} and {@link
   * Variant} result types.
   */
  sealed interface ReadResult permits ReadResult.Inclusion, ReadResult.Variant {
    /** Returns the materialization identifier. */
    String materialization();

    /** Returns the unit identifier (user ID, session ID, etc.). */
    String unit();

    /**
     * Result indicating whether a unit is included in a materialized segment.
     *
     * <p>Used for custom targeting via materialized segments - efficient lookup to determine if a
     * unit (user, session, etc.) is part of a precomputed target set.
     *
     * @param materialization the materialization context identifier
     * @param unit the entity identifier (user ID, session ID, etc.)
     * @param included true if the unit is included in the materialized segment
     */
    record Inclusion(String materialization, String unit, boolean included) implements ReadResult {}

    /**
     * Result containing the variant assignment for a unit and rule.
     *
     * <p>Used for sticky assignments - returns the previously assigned variant for a unit and
     * targeting rule combination.
     *
     * @param materialization the materialization context identifier
     * @param unit the entity identifier (user ID, session ID, etc.)
     * @param rule the targeting rule identifier
     * @param variant the assigned variant name, or empty if no assignment exists
     */
    record Variant(String materialization, String unit, String rule, Optional<String> variant)
        implements ReadResult {}
  }

  /**
   * Represents a read operation to query materialization data.
   *
   * <p>This sealed interface ensures type-safety and is implemented by {@link Inclusion} and {@link
   * Variant} operation types.
   */
  sealed interface ReadOp permits ReadOp.Inclusion, ReadOp.Variant {
    /** Returns the materialization identifier. */
    String materialization();

    /** Returns the unit identifier (user ID, session ID, etc.). */
    String unit();

    /**
     * Query operation to check if a unit is included in a materialized segment.
     *
     * <p>Used for custom targeting to efficiently determine if a unit is part of a precomputed
     * target set from a dataset.
     *
     * @param materialization the materialization context identifier
     * @param unit the entity identifier (user ID, session ID, etc.)
     */
    record Inclusion(String materialization, String unit) implements ReadOp {
      /**
       * Converts a boolean result into a properly typed ReadResult.
       *
       * @param included whether the unit is included in the materialized segment
       * @return the typed read result
       */
      public ReadResult.Inclusion toResult(boolean included) {
        return new ReadResult.Inclusion(materialization, unit, included);
      }
    }

    /**
     * Query operation to retrieve the variant assignment for a unit and rule.
     *
     * <p>Used for sticky assignments to fetch the previously assigned variant.
     *
     * @param materialization the materialization context identifier
     * @param unit the entity identifier (user ID, session ID, etc.)
     * @param rule the targeting rule identifier
     */
    record Variant(String materialization, String unit, String rule) implements ReadOp {
      /**
       * Converts an optional variant into a properly typed ReadResult.
       *
       * @param variant the assigned variant name, or empty if no assignment exists
       * @return the typed read result
       */
      public ReadResult.Variant toResult(Optional<String> variant) {
        return new ReadResult.Variant(materialization, unit, rule, variant);
      }
    }
  }
}
