package com.spotify.confidence.android

import java.util.Optional
import java.util.concurrent.CompletableFuture

/**
 * Storage abstraction for materialization data used in flag resolution.
 *
 * Materializations support two key use cases:
 * - **Sticky Assignments**: Maintain consistent variant assignments across evaluations
 *   even when targeting attributes change.
 * - **Custom Targeting via Materialized Segments**: Precomputed sets of identifiers
 *   from datasets that should be targeted.
 *
 * Default Behavior: By default, the provider uses [UnsupportedMaterializationStore]
 * which triggers remote resolution via gRPC to the Confidence service.
 *
 * Thread Safety: Implementations must be thread-safe as they may be called
 * concurrently from multiple threads resolving flags in parallel.
 */
interface MaterializationStore {

    /**
     * Performs a batch read of materialization data.
     *
     * @param ops the list of read operations to perform
     * @return a CompletableFuture that completes with the read results
     * @throws MaterializationNotSupportedException if the store doesn't support reads
     */
    @Throws(MaterializationNotSupportedException::class)
    fun read(ops: List<ReadOp>): CompletableFuture<List<ReadResult>>

    /**
     * Performs a batch write of materialization data.
     *
     * @param ops the set of write operations to perform
     * @return a CompletableFuture that completes when all writes are finished
     * @throws MaterializationNotSupportedException by default if not overridden
     */
    @Throws(MaterializationNotSupportedException::class)
    fun write(ops: Set<WriteOp>): CompletableFuture<Void?> {
        throw MaterializationNotSupportedException()
    }

    /**
     * Represents a write operation to store materialization data.
     */
    sealed interface WriteOp {
        val materialization: String
        val unit: String

        /**
         * A variant assignment write operation.
         */
        data class Variant(
            override val materialization: String,
            override val unit: String,
            val rule: String,
            val variant: String
        ) : WriteOp
    }

    /**
     * Represents the result of a read operation.
     */
    sealed interface ReadResult {
        val materialization: String
        val unit: String

        /**
         * Result indicating whether a unit is included in a materialized segment.
         */
        data class Inclusion(
            override val materialization: String,
            override val unit: String,
            val included: Boolean
        ) : ReadResult

        /**
         * Result containing the variant assignment for a unit and rule.
         */
        data class Variant(
            override val materialization: String,
            override val unit: String,
            val rule: String,
            val variant: Optional<String>
        ) : ReadResult
    }

    /**
     * Represents a read operation to query materialization data.
     */
    sealed interface ReadOp {
        val materialization: String
        val unit: String

        /**
         * Query operation to check if a unit is included in a materialized segment.
         */
        data class Inclusion(
            override val materialization: String,
            override val unit: String
        ) : ReadOp {
            fun toResult(included: Boolean): ReadResult.Inclusion {
                return ReadResult.Inclusion(materialization, unit, included)
            }
        }

        /**
         * Query operation to retrieve the variant assignment for a unit and rule.
         */
        data class Variant(
            override val materialization: String,
            override val unit: String,
            val rule: String
        ) : ReadOp {
            fun toResult(variant: Optional<String>): ReadResult.Variant {
                return ReadResult.Variant(materialization, unit, rule, variant)
            }
        }
    }
}

/**
 * Exception thrown when materialization operations are not supported.
 */
class MaterializationNotSupportedException : Exception("Materialization not supported")
