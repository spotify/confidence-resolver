package com.spotify.confidence.android

import java.util.concurrent.CompletableFuture

/**
 * Default materialization store that throws [MaterializationNotSupportedException]
 * for all operations, triggering fallback to remote gRPC resolution.
 */
internal class UnsupportedMaterializationStore : MaterializationStore {

    override fun read(ops: List<MaterializationStore.ReadOp>): CompletableFuture<List<MaterializationStore.ReadResult>> {
        throw MaterializationNotSupportedException()
    }

    override fun write(ops: Set<MaterializationStore.WriteOp>): CompletableFuture<Void?> {
        throw MaterializationNotSupportedException()
    }
}
