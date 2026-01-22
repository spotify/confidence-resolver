package main

import (
	"context"
	"log"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
)

func main() {
	ctx := context.Background()

	clientSecret := getEnvOrDefault("CONFIDENCE_CLIENT_SECRET", "CLIENT_SECRET")

	if clientSecret == "CLIENT_SECRET" {
		log.Fatalf("ERROR: Placeholder credentials detected. Please set environment variables:\n" +
			"  - CONFIDENCE_CLIENT_SECRET\n\n" +
			"Example:\n" +
			"  export CONFIDENCE_CLIENT_SECRET=\"your-client-secret\"\n")
	}

	log.Println("Starting Confidence OpenFeature Local Provider Demo")
	log.Println("")

	log.Println("Creating Confidence provider...")
	provider, err := confidence.NewProvider(ctx, confidence.ProviderConfig{
		ClientSecret:                  clientSecret,
		UseRemoteMaterializationStore: true,
		StatePollInterval:             30 * time.Second,
		LogPollInterval:               2 * time.Minute,
	})

	if err != nil {
		log.Fatalf("Failed to create provider: %v", err)
	}
	defer openfeature.Shutdown()
	log.Println("Confidence provider created successfully")

	// Register with OpenFeature
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		return
	}
	log.Println("OpenFeature provider registered")
	log.Println("")

	// Create OpenFeature client
	client := openfeature.NewClient("demo-app")

	// Demo: Evaluate flags with multiple concurrent threads
	log.Println("=== Flag Evaluation Demo with 10 Concurrent Threads ===")
	log.Println("")

	// Create evaluation context
	evalCtx := openfeature.NewEvaluationContext(
		"targeting_key_123",
		map[string]interface{}{
			"sticky":     true,
			"visitor_id": "vahid",
		},
	)

	// Run 5 concurrent threads continuously for 5 second
	var wg sync.WaitGroup
	numThreads := 2
	runDuration := 5 * time.Second

	log.Printf("Starting %d threads to run for %v to test reload and flush...", numThreads, runDuration)
	log.Println("")

	startTime := time.Now()
	stopTime := startTime.Add(runDuration)

	// Shared counters for throughput calculation
	var totalSuccess, totalErrors int64

	for i := 0; i < numThreads; i++ {
		wg.Add(1)
		threadID := i
		go func() {
			defer wg.Done()

			var successCount, errorCount int64
			iteration := 0

			for time.Now().Before(stopTime) {
				// Use ObjectValueDetails to get the full flag object
				result, err := client.ObjectValueDetails(ctx, "web-sdk-e2e-flag", map[string]interface{}{}, evalCtx)
				if err != nil {
					errorCount++
					if iteration == 0 { // Only log first error per thread
						log.Printf("Thread %d: Error: %v", threadID, err)
					}
				} else {
					successCount++
					if iteration == 0 { // Only log first success per thread
						log.Printf("Thread %d: First result - Value: %+v, Variant: %s, Reason: %s",
							threadID, result.Value, result.Variant, result.Reason)
					}
				}
				iteration++

				// Small sleep to avoid tight loop
				time.Sleep(1 * time.Millisecond)
				//break
			}

			// Update shared counters atomically
			atomic.AddInt64(&totalSuccess, successCount)
			atomic.AddInt64(&totalErrors, errorCount)

			log.Printf("Thread %d complete after %v: %d successes, %d errors (%d total iterations)",
				threadID, time.Since(startTime), successCount, errorCount, iteration)
		}()
	}

	// Wait for all threads to complete
	wg.Wait()

	duration := time.Since(startTime)
	totalRequests := totalSuccess + totalErrors
	throughputPerSecond := float64(totalRequests) / duration.Seconds()

	log.Println("")
	log.Println("=== Demo Complete ===")
	log.Printf("Total time: %v", duration)
	log.Printf("Throughput: %.2f requests/second", throughputPerSecond)
	log.Printf("Average latency: %.2f ms/request", duration.Seconds()*1000/float64(totalRequests))
	log.Println("Check logs above for per-thread statistics and state reload/flush messages")
	log.Println("")

	log.Println("At the end of main... shutting down...")
}

func getEnvOrDefault(key, defaultValue string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return defaultValue
}

func printBooleanResult(result openfeature.BooleanEvaluationDetails) {
	log.Printf("  Value: %v", result.Value)
	log.Printf("  Variant: %s", result.Variant)
	log.Printf("  Reason: %s", result.Reason)
	if result.ErrorCode != "" {
		log.Printf("  Error Code: %s", result.ErrorCode)
		log.Printf("  Error Message: %s", result.ErrorMessage)
	}
}

func printStringResult(result openfeature.StringEvaluationDetails) {
	log.Printf("  Value: %s", result.Value)
	log.Printf("  Variant: %s", result.Variant)
	log.Printf("  Reason: %s", result.Reason)
	if result.ErrorCode != "" {
		log.Printf("  Error Code: %s", result.ErrorCode)
		log.Printf("  Error Message: %s", result.ErrorMessage)
	}
}

func printIntResult(result openfeature.IntEvaluationDetails) {
	log.Printf("  Value: %d", result.Value)
	log.Printf("  Variant: %s", result.Variant)
	log.Printf("  Reason: %s", result.Reason)
	if result.ErrorCode != "" {
		log.Printf("  Error Code: %s", result.ErrorCode)
		log.Printf("  Error Message: %s", result.ErrorMessage)
	}
}

func printFloatResult(result openfeature.FloatEvaluationDetails) {
	log.Printf("  Value: %f", result.Value)
	log.Printf("  Variant: %s", result.Variant)
	log.Printf("  Reason: %s", result.Reason)
	if result.ErrorCode != "" {
		log.Printf("  Error Code: %s", result.ErrorCode)
		log.Printf("  Error Message: %s", result.ErrorMessage)
	}
}

func printObjectResult(result openfeature.InterfaceEvaluationDetails) {
	log.Printf("  Value: %+v", result.Value)
	log.Printf("  Variant: %s", result.Variant)
	log.Printf("  Reason: %s", result.Reason)
	if result.ErrorCode != "" {
		log.Printf("  Error Code: %s", result.ErrorCode)
		log.Printf("  Error Message: %s", result.ErrorMessage)
	}
}
