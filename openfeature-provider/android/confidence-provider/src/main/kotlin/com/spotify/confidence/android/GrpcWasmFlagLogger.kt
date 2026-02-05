package com.spotify.confidence.android

import android.util.Log
import com.spotify.confidence.flags.resolver.v1.InternalFlagLoggerServiceGrpc
import com.spotify.confidence.flags.resolver.v1.WriteFlagLogsRequest
import io.grpc.ManagedChannel
import io.grpc.Metadata
import io.grpc.stub.MetadataUtils
import java.time.Duration
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Flag logger that sends flag resolution events to the Confidence service via gRPC.
 */
internal class GrpcWasmFlagLogger(
    private val clientSecret: String,
    private val channelFactory: ChannelFactory = DefaultChannelFactory(),
    private val shutdownTimeout: Duration = DEFAULT_SHUTDOWN_TIMEOUT
) : WasmFlagLogger {

    private val executorService: ExecutorService = Executors.newCachedThreadPool()
    private var channel: ManagedChannel? = null

    companion object {
        private const val TAG = "GrpcWasmFlagLogger"
        private val DEFAULT_SHUTDOWN_TIMEOUT = Duration.ofSeconds(10)
        private const val AUTH_HEADER = "authorization"
    }

    private fun getOrCreateChannel(): ManagedChannel {
        return channel ?: channelFactory.create().also { channel = it }
    }

    private fun createStub(): InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub {
        val metadata = Metadata()
        metadata.put(
            Metadata.Key.of(AUTH_HEADER, Metadata.ASCII_STRING_MARSHALLER),
            "ClientSecret $clientSecret"
        )

        return InternalFlagLoggerServiceGrpc.newBlockingStub(getOrCreateChannel())
            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(metadata))
            .withDeadlineAfter(30, TimeUnit.SECONDS)
    }

    override fun write(logData: ByteArray) {
        if (logData.isEmpty()) {
            Log.d(TAG, "Skipping empty flag log")
            return
        }

        executorService.submit {
            try {
                val request = WriteFlagLogsRequest.parseFrom(logData)
                Log.d(TAG, "Sending ${logData.size} bytes of log data (${request.flagAssignedCount} assignments)")

                val stub = createStub()
                stub.clientWriteFlagLogs(request)

                Log.d(TAG, "Successfully sent flag logs")
            } catch (e: Exception) {
                Log.e(TAG, "Failed to write flag logs", e)
            }
        }
    }

    override fun writeSync(logData: ByteArray) {
        if (logData.isEmpty()) {
            Log.d(TAG, "Skipping empty flag log")
            return
        }

        try {
            val request = WriteFlagLogsRequest.parseFrom(logData)
            Log.d(TAG, "Synchronously sending ${logData.size} bytes of log data")

            val stub = createStub()
            stub.clientWriteFlagLogs(request)

            Log.d(TAG, "Successfully sent flag logs synchronously")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to write flag logs synchronously", e)
        }
    }

    override fun shutdown() {
        executorService.shutdown()
        try {
            if (!executorService.awaitTermination(shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS)) {
                Log.w(TAG, "Flag logger executor did not terminate within ${shutdownTimeout.seconds} seconds")
                executorService.shutdownNow()
            } else {
                Log.d(TAG, "Flag logger executor terminated gracefully")
            }
        } catch (e: InterruptedException) {
            Log.w(TAG, "Interrupted while waiting for flag logger shutdown", e)
            executorService.shutdownNow()
            Thread.currentThread().interrupt()
        }

        channel?.let { ch ->
            try {
                ch.shutdown()
                if (!ch.awaitTermination(5, TimeUnit.SECONDS)) {
                    ch.shutdownNow()
                }
            } catch (e: Exception) {
                Log.w(TAG, "Error shutting down gRPC channel", e)
            }
        }
    }
}

/**
 * Factory for creating gRPC channels.
 */
interface ChannelFactory {
    fun create(): ManagedChannel
}

/**
 * Default channel factory that creates a channel to the Confidence edge service.
 */
class DefaultChannelFactory : ChannelFactory {
    companion object {
        private const val CONFIDENCE_HOST = "edge-grpc.spotify.com"
        private const val CONFIDENCE_PORT = 443
    }

    override fun create(): ManagedChannel {
        return io.grpc.okhttp.OkHttpChannelBuilder
            .forAddress(CONFIDENCE_HOST, CONFIDENCE_PORT)
            .useTransportSecurity()
            .build()
    }
}
