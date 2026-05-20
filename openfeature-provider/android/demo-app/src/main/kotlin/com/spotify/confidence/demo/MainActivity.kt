package com.spotify.confidence.demo

import android.os.Bundle
import android.util.Log
import android.view.View
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import com.spotify.confidence.android.ConfidenceLocalProvider
import com.spotify.confidence.android.LocalProviderConfig
import com.spotify.confidence.demo.databinding.ActivityMainBinding
import dev.openfeature.kotlin.sdk.ImmutableContext
import dev.openfeature.kotlin.sdk.OpenFeatureAPI
import dev.openfeature.kotlin.sdk.Value
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

class MainActivity : AppCompatActivity() {

    private lateinit var binding: ActivityMainBinding
    private val logBuilder = StringBuilder()
    private val dateFormat = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)

    // Benchmark state
    private var benchmarkJob: Job? = null
    private val isBenchmarkRunning = AtomicBoolean(false)
    private val totalRequests = AtomicInteger(0)
    private val totalErrors = AtomicInteger(0)
    private val totalLatencyNanos = AtomicLong(0)
    private val minLatencyNanos = AtomicLong(Long.MAX_VALUE)
    private val maxLatencyNanos = AtomicLong(0)
    private var benchmarkStartTime = 0L
    private val latencyHistogram = mutableListOf<Long>() // Store latencies for percentile calculation

    companion object {
        private const val TAG = "ConfidenceDemo"
        // Set via BuildConfig from local.properties: confidence.clientSecret=YOUR_SECRET
        private const val CLIENT_SECRET = BuildConfig.CONFIDENCE_CLIENT_SECRET
        private const val FLAG_KEY = "mattias-boolean-flag.enabled"
        private const val UI_UPDATE_INTERVAL_MS = 100L
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        setupButtons()
        log("Demo app started")
        log("Client Secret: ${CLIENT_SECRET.take(8)}...")
        log("Flag to resolve: $FLAG_KEY")
    }

    private fun setupButtons() {
        binding.initButton.setOnClickListener {
            initializeProvider()
        }

        binding.resolveButton.setOnClickListener {
            resolveFlag()
        }

        binding.benchmarkButton.setOnClickListener {
            if (isBenchmarkRunning.get()) {
                stopBenchmark()
            } else {
                startBenchmark()
            }
        }
    }

    private fun initializeProvider() {
        binding.initButton.isEnabled = false
        binding.statusText.text = "Status: Initializing..."
        log("Initializing provider...")

        lifecycleScope.launch {
            try {
                withContext(Dispatchers.IO) {
                    val config = LocalProviderConfig.Builder()
                        .build()

                    val provider = ConfidenceLocalProvider.create(
                        clientSecret = CLIENT_SECRET,
                        config = config
                    )

                    log("Provider created: ${provider.metadata.name}")

                    val context = ImmutableContext(
                        targetingKey = "vahid",
                        attributes = mapOf(
                            "user_id" to Value.String("vahid"),
                            "country" to Value.String("SE"),
                            "platform" to Value.String("android"),
                            "app_version" to Value.String("1.0.0")
                        )
                    )

                    log("Setting evaluation context: targeting_key=vahid")
                    OpenFeatureAPI.setEvaluationContext(context)

                    log("Calling provider.initialize()...")
                    provider.initialize(context)

                    OpenFeatureAPI.setProvider(provider)
                    log("Provider set successfully!")
                }

                withContext(Dispatchers.Main) {
                    binding.statusText.text = "Status: Ready"
                    binding.statusText.setTextColor(getColor(android.R.color.holo_green_dark))
                    binding.resolveButton.isEnabled = true
                    binding.benchmarkButton.isEnabled = true
                    binding.initButton.text = "Re-init"
                    binding.initButton.isEnabled = true
                    log("Provider initialization complete!")
                }

            } catch (e: Exception) {
                Log.e(TAG, "Failed to initialize", e)
                withContext(Dispatchers.Main) {
                    binding.statusText.text = "Status: Error"
                    binding.statusText.setTextColor(getColor(android.R.color.holo_red_dark))
                    binding.initButton.isEnabled = true
                    log("ERROR: ${e.message}")
                }
            }
        }
    }

    private fun resolveFlag() {
        binding.resolveButton.isEnabled = false
        log("Resolving flag: $FLAG_KEY")

        lifecycleScope.launch {
            try {
                val client = OpenFeatureAPI.getClient()
                val startTime = System.currentTimeMillis()

                val evaluation = withContext(Dispatchers.IO) {
                    client.getBooleanDetails(FLAG_KEY, false)
                }

                val duration = System.currentTimeMillis() - startTime

                withContext(Dispatchers.Main) {
                    binding.flagValueText.text = "Value: ${evaluation.value}"
                    binding.variantText.text = "Variant: ${evaluation.variant ?: "N/A"}"
                    binding.reasonText.text = "Reason: ${evaluation.reason ?: "N/A"}"

                    val color = if (evaluation.value) {
                        getColor(android.R.color.holo_green_dark)
                    } else {
                        getColor(android.R.color.holo_red_dark)
                    }
                    binding.flagValueText.setTextColor(color)

                    log("=== RESOLUTION RESULT ===")
                    log("Value: ${evaluation.value}, Variant: ${evaluation.variant}, Duration: ${duration}ms")
                    log("=========================")

                    binding.resolveButton.isEnabled = true
                }

            } catch (e: Exception) {
                Log.e(TAG, "Failed to resolve flag", e)
                withContext(Dispatchers.Main) {
                    binding.flagValueText.text = "Value: ERROR"
                    binding.flagValueText.setTextColor(getColor(android.R.color.holo_red_dark))
                    log("ERROR resolving flag: ${e.message}")
                    binding.resolveButton.isEnabled = true
                }
            }
        }
    }

    private fun startBenchmark() {
        // Reset stats
        totalRequests.set(0)
        totalErrors.set(0)
        totalLatencyNanos.set(0)
        minLatencyNanos.set(Long.MAX_VALUE)
        maxLatencyNanos.set(0)
        synchronized(latencyHistogram) {
            latencyHistogram.clear()
        }
        benchmarkStartTime = System.nanoTime()

        isBenchmarkRunning.set(true)
        binding.benchmarkButton.text = "STOP"
        binding.benchmarkButton.setBackgroundColor(getColor(android.R.color.holo_red_dark))
        binding.benchmarkCard.visibility = View.VISIBLE
        binding.resolveButton.isEnabled = false
        binding.initButton.isEnabled = false

        log("=== BENCHMARK STARTED ===")
        log("Press STOP to end the benchmark")

        // Start the benchmark worker
        benchmarkJob = lifecycleScope.launch {
            val client = OpenFeatureAPI.getClient()

            // UI update job
            val uiUpdateJob = launch {
                while (isActive && isBenchmarkRunning.get()) {
                    updateBenchmarkUI()
                    delay(UI_UPDATE_INTERVAL_MS)
                }
            }

            // Benchmark worker - run continuously until stopped
            withContext(Dispatchers.IO) {
                while (isBenchmarkRunning.get()) {
                    val startNanos = System.nanoTime()
                    try {
                        client.getBooleanDetails(FLAG_KEY, false)
                        val latencyNanos = System.nanoTime() - startNanos

                        totalRequests.incrementAndGet()
                        totalLatencyNanos.addAndGet(latencyNanos)

                        // Update min/max atomically
                        var current = minLatencyNanos.get()
                        while (latencyNanos < current) {
                            if (minLatencyNanos.compareAndSet(current, latencyNanos)) break
                            current = minLatencyNanos.get()
                        }

                        current = maxLatencyNanos.get()
                        while (latencyNanos > current) {
                            if (maxLatencyNanos.compareAndSet(current, latencyNanos)) break
                            current = maxLatencyNanos.get()
                        }

                        // Store for percentile calculation (limit to last 10000 samples)
                        synchronized(latencyHistogram) {
                            if (latencyHistogram.size >= 10000) {
                                latencyHistogram.removeAt(0)
                            }
                            latencyHistogram.add(latencyNanos)
                        }

                    } catch (e: Exception) {
                        totalErrors.incrementAndGet()
                    }
                }
            }

            uiUpdateJob.cancel()
        }
    }

    private fun stopBenchmark() {
        isBenchmarkRunning.set(false)
        benchmarkJob?.cancel()

        binding.benchmarkButton.text = "Benchmark"
        binding.benchmarkButton.setBackgroundColor(getColor(android.R.color.holo_blue_dark))
        binding.resolveButton.isEnabled = true
        binding.initButton.isEnabled = true

        // Final UI update
        updateBenchmarkUI()
        binding.benchmarkStatusText.text = "Stopped"
        binding.benchmarkStatusText.setTextColor(getColor(android.R.color.darker_gray))

        // Log final results
        val requests = totalRequests.get()
        val errors = totalErrors.get()
        val elapsedSeconds = (System.nanoTime() - benchmarkStartTime) / 1_000_000_000.0
        val rps = if (elapsedSeconds > 0) requests / elapsedSeconds else 0.0
        val avgLatencyMs = if (requests > 0) (totalLatencyNanos.get() / requests) / 1_000_000.0 else 0.0

        val (p50, p95, p99) = calculatePercentiles()

        log("=== BENCHMARK COMPLETED ===")
        log("Total Requests: $requests")
        log("Total Errors: $errors")
        log("Duration: %.2f seconds".format(elapsedSeconds))
        log("RPS: %.1f".format(rps))
        log("Avg Latency: %.2f ms".format(avgLatencyMs))
        log("Min Latency: %.2f ms".format(minLatencyNanos.get() / 1_000_000.0))
        log("Max Latency: %.2f ms".format(maxLatencyNanos.get() / 1_000_000.0))
        log("P50: %.2f ms, P95: %.2f ms, P99: %.2f ms".format(p50, p95, p99))
        log("===========================")
    }

    private fun updateBenchmarkUI() {
        runOnUiThread {
            val requests = totalRequests.get()
            val errors = totalErrors.get()
            val elapsedNanos = System.nanoTime() - benchmarkStartTime
            val elapsedSeconds = elapsedNanos / 1_000_000_000.0

            // Calculate RPS
            val rps = if (elapsedSeconds > 0) requests / elapsedSeconds else 0.0

            // Calculate average latency
            val avgLatencyMs = if (requests > 0) {
                (totalLatencyNanos.get() / requests) / 1_000_000.0
            } else {
                0.0
            }

            // Calculate percentiles
            val (p50, p95, p99) = calculatePercentiles()

            // Update UI
            binding.totalRequestsText.text = "%,d".format(requests)
            binding.rpsText.text = "%.1f".format(rps)
            binding.avgLatencyText.text = "%.2f ms".format(avgLatencyMs)

            val minMs = if (minLatencyNanos.get() == Long.MAX_VALUE) 0.0 else minLatencyNanos.get() / 1_000_000.0
            val maxMs = maxLatencyNanos.get() / 1_000_000.0
            binding.minMaxLatencyText.text = "%.1f / %.1f ms".format(minMs, maxMs)

            binding.percentilesText.text = "%.1f / %.1f / %.1f ms".format(p50, p95, p99)
            binding.errorsText.text = errors.toString()

            if (isBenchmarkRunning.get()) {
                binding.benchmarkStatusText.text = "Running... (%.1fs)".format(elapsedSeconds)
                binding.benchmarkStatusText.setTextColor(getColor(android.R.color.holo_blue_dark))
            }
        }
    }

    private fun calculatePercentiles(): Triple<Double, Double, Double> {
        synchronized(latencyHistogram) {
            if (latencyHistogram.isEmpty()) {
                return Triple(0.0, 0.0, 0.0)
            }

            val sorted = latencyHistogram.sorted()
            val size = sorted.size

            val p50Index = (size * 0.50).toInt().coerceIn(0, size - 1)
            val p95Index = (size * 0.95).toInt().coerceIn(0, size - 1)
            val p99Index = (size * 0.99).toInt().coerceIn(0, size - 1)

            return Triple(
                sorted[p50Index] / 1_000_000.0,
                sorted[p95Index] / 1_000_000.0,
                sorted[p99Index] / 1_000_000.0
            )
        }
    }

    private fun log(message: String) {
        val timestamp = dateFormat.format(Date())
        val logLine = "[$timestamp] $message\n"
        Log.d(TAG, message)

        runOnUiThread {
            logBuilder.append(logLine)
            // Keep log size manageable
            if (logBuilder.length > 5000) {
                logBuilder.delete(0, logBuilder.length - 4000)
            }
            binding.logText.text = logBuilder.toString()
        }
    }
}
