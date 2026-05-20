package com.spotify.confidence.android

/**
 * Interface for logging flag resolution events to the Confidence service.
 */
internal interface WasmFlagLogger {
    /**
     * Asynchronously writes flag logs (raw bytes from WASM).
     */
    fun write(logData: ByteArray)

    /**
     * Synchronously writes flag logs, blocking until complete.
     */
    fun writeSync(logData: ByteArray)

    /**
     * Shuts down the logger, waiting for pending writes to complete.
     */
    fun shutdown()
}
