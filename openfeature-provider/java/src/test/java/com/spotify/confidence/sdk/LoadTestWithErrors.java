package com.spotify.confidence.sdk;

import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Load test that continuously resolves flags at ~100/second.
 * 
 * Out of every 100 resolves:
 * - 97 are valid flags (web-sdk-e2e-flag.bool)
 * - 3 are invalid flags (non-existent-flag-X) to trigger FLAG_NOT_FOUND errors
 * 
 * Run with: mvn test -Dtest=LoadTestWithErrors#runLoadTest -DfailIfNoTests=false
 * 
 * Or run the main method directly.
 */
public class LoadTestWithErrors {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "load-test-user";
  
  // Valid flag that exists
  private static final String VALID_FLAG = "web-sdk-e2e-flag.bool";
  
  // Invalid flags that will trigger FLAG_NOT_FOUND
  private static final String[] INVALID_FLAGS = {
      "non-existent-flag-1.bool",
      "non-existent-flag-2.bool",
      "non-existent-flag-3.bool"
  };
  
  private static final int RESOLVES_PER_SECOND = 100;
  private static final int ERRORS_PER_SECOND = 3;
  private static final int VALID_PER_SECOND = RESOLVES_PER_SECOND - ERRORS_PER_SECOND;
  
  public static void main(String[] args) throws Exception {
    System.out.println("Starting load test with error injection...");
    System.out.println("  - " + RESOLVES_PER_SECOND + " resolves/second");
    System.out.println("  - " + VALID_PER_SECOND + " valid flags");
    System.out.println("  - " + ERRORS_PER_SECOND + " invalid flags (FLAG_NOT_FOUND errors)");
    System.out.println();
    
    runLoadTestContinuously();
  }
  
  public static void runLoadTestContinuously() throws Exception {
    // Set up the provider
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "4");
    
    final var stateProvider =
        new FlagsAdminStateFetcher(FLAG_CLIENT_SECRET, new DefaultHttpClientFactory());
    System.out.println("Fetching flag state...");
    stateProvider.reload();
    System.out.println("Flag state loaded.");
    
    // Use the real flag logger that sends to backend via gRPC
    final var flagLogger = new GrpcWasmFlagLogger(FLAG_CLIENT_SECRET, new DefaultChannelFactory());
    
    final var provider =
        new OpenFeatureLocalResolveProvider(
            stateProvider,
            FLAG_CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            flagLogger);
    
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
    
    // Set evaluation context
    final EvaluationContext context = new MutableContext(TARGETING_KEY).add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);
    
    final Client client = OpenFeatureAPI.getInstance().getClient();
    
    // Counters for stats
    final AtomicLong totalResolves = new AtomicLong(0);
    final AtomicLong validResolves = new AtomicLong(0);
    final AtomicLong errorResolves = new AtomicLong(0);
    
    // Schedule resolves at fixed rate
    final ScheduledExecutorService executor = Executors.newScheduledThreadPool(4);
    
    // Calculate interval between resolves (in microseconds)
    final long intervalMicros = 1_000_000 / RESOLVES_PER_SECOND; // 10ms for 100/s
    
    System.out.println("Starting resolves (interval: " + intervalMicros + " microseconds)...");
    System.out.println("Press Ctrl+C to stop");
    System.out.println();
    
    final long startTime = System.currentTimeMillis();
    final AtomicLong resolveCounter = new AtomicLong(0);
    
    // Schedule the resolve task
    executor.scheduleAtFixedRate(() -> {
      try {
        long count = resolveCounter.incrementAndGet();
        
        // Every RESOLVES_PER_SECOND resolves, ERRORS_PER_SECOND should be errors
        // So error on resolves: 1, 34, 67 (roughly evenly distributed)
        boolean shouldError = (count % (RESOLVES_PER_SECOND / ERRORS_PER_SECOND)) == 1;
        
        if (shouldError) {
          // Resolve an invalid flag - triggers FLAG_NOT_FOUND
          String invalidFlag = INVALID_FLAGS[(int)(errorResolves.get() % INVALID_FLAGS.length)];
          client.getBooleanValue(invalidFlag, false);
          errorResolves.incrementAndGet();
        } else {
          // Resolve a valid flag
          client.getBooleanValue(VALID_FLAG, true);
          validResolves.incrementAndGet();
        }
        
        totalResolves.incrementAndGet();
        
      } catch (Exception e) {
        // Ignore errors - they're expected for invalid flags
      }
    }, 0, intervalMicros, TimeUnit.MICROSECONDS);
    
    // Print stats every second
    executor.scheduleAtFixedRate(() -> {
      long elapsed = (System.currentTimeMillis() - startTime) / 1000;
      long total = totalResolves.get();
      long valid = validResolves.get();
      long errors = errorResolves.get();
      double rate = elapsed > 0 ? (double) total / elapsed : 0;
      
      System.out.printf("[%3ds] Total: %6d | Valid: %6d | Errors: %5d | Rate: %.1f/s%n",
          elapsed, total, valid, errors, rate);
    }, 1, 1, TimeUnit.SECONDS);
    
    // Add shutdown hook to handle Ctrl+C gracefully
    Runtime.getRuntime().addShutdownHook(new Thread(() -> {
      System.out.println();
      System.out.println("Stopping load test...");
      executor.shutdown();
      try {
        executor.awaitTermination(5, TimeUnit.SECONDS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
      
      // Shutdown provider to flush logs
      System.out.println("Flushing logs to backend...");
      provider.shutdown();
      
      // Final stats
      long elapsed = (System.currentTimeMillis() - startTime) / 1000;
      System.out.println();
      System.out.println("=== Final Stats ===");
      System.out.println("Duration: " + elapsed + " seconds");
      System.out.println("Total resolves: " + totalResolves.get());
      System.out.println("Valid resolves: " + validResolves.get());
      System.out.println("Error resolves: " + errorResolves.get());
      System.out.println("Average rate: " + (totalResolves.get() / Math.max(1, elapsed)) + "/s");
      System.out.println("==================");
    }));
    
    // Keep main thread alive indefinitely
    Thread.currentThread().join();
  }

  public static void runLoadTest(int durationSeconds) throws Exception {
    // Set up the provider
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "4");
    
    final var stateProvider =
        new FlagsAdminStateFetcher(FLAG_CLIENT_SECRET, new DefaultHttpClientFactory());
    System.out.println("Fetching flag state...");
    stateProvider.reload();
    System.out.println("Flag state loaded.");
    
    // Use the real flag logger that sends to backend via gRPC
    final var flagLogger = new GrpcWasmFlagLogger(FLAG_CLIENT_SECRET, new DefaultChannelFactory());
    
    final var provider =
        new OpenFeatureLocalResolveProvider(
            stateProvider,
            FLAG_CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            flagLogger);
    
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
    
    // Set evaluation context
    final EvaluationContext context = new MutableContext(TARGETING_KEY).add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);
    
    final Client client = OpenFeatureAPI.getInstance().getClient();
    
    // Counters for stats
    final AtomicLong totalResolves = new AtomicLong(0);
    final AtomicLong validResolves = new AtomicLong(0);
    final AtomicLong errorResolves = new AtomicLong(0);
    
    // Schedule resolves at fixed rate
    final ScheduledExecutorService executor = Executors.newScheduledThreadPool(4);
    
    // Calculate interval between resolves (in microseconds)
    final long intervalMicros = 1_000_000 / RESOLVES_PER_SECOND; // 10ms for 100/s
    
    System.out.println("Starting resolves (interval: " + intervalMicros + " microseconds)...");
    System.out.println("Press Ctrl+C to stop");
    System.out.println();
    
    final long startTime = System.currentTimeMillis();
    final AtomicLong resolveCounter = new AtomicLong(0);
    
    // Schedule the resolve task
    executor.scheduleAtFixedRate(() -> {
      try {
        long count = resolveCounter.incrementAndGet();
        
        // Every RESOLVES_PER_SECOND resolves, ERRORS_PER_SECOND should be errors
        // So error on resolves: 1, 34, 67 (roughly evenly distributed)
        boolean shouldError = (count % (RESOLVES_PER_SECOND / ERRORS_PER_SECOND)) == 1;
        
        if (shouldError) {
          // Resolve an invalid flag - triggers FLAG_NOT_FOUND
          String invalidFlag = INVALID_FLAGS[(int)(errorResolves.get() % INVALID_FLAGS.length)];
          client.getBooleanValue(invalidFlag, false);
          errorResolves.incrementAndGet();
        } else {
          // Resolve a valid flag
          client.getBooleanValue(VALID_FLAG, true);
          validResolves.incrementAndGet();
        }
        
        totalResolves.incrementAndGet();
        
      } catch (Exception e) {
        // Ignore errors - they're expected for invalid flags
      }
    }, 0, intervalMicros, TimeUnit.MICROSECONDS);
    
    // Print stats every second
    executor.scheduleAtFixedRate(() -> {
      long elapsed = (System.currentTimeMillis() - startTime) / 1000;
      long total = totalResolves.get();
      long valid = validResolves.get();
      long errors = errorResolves.get();
      double rate = elapsed > 0 ? (double) total / elapsed : 0;
      
      System.out.printf("[%3ds] Total: %6d | Valid: %6d | Errors: %5d | Rate: %.1f/s%n",
          elapsed, total, valid, errors, rate);
    }, 1, 1, TimeUnit.SECONDS);
    
    // Run for specified duration
    Thread.sleep(durationSeconds * 1000L);
    
    System.out.println();
    System.out.println("Stopping load test...");
    executor.shutdown();
    executor.awaitTermination(5, TimeUnit.SECONDS);
    
    // Shutdown provider to flush logs
    System.out.println("Flushing logs to backend...");
    provider.shutdown();
    
    // Final stats
    long elapsed = (System.currentTimeMillis() - startTime) / 1000;
    System.out.println();
    System.out.println("=== Final Stats ===");
    System.out.println("Duration: " + elapsed + " seconds");
    System.out.println("Total resolves: " + totalResolves.get());
    System.out.println("Valid resolves: " + validResolves.get());
    System.out.println("Error resolves: " + errorResolves.get());
    System.out.println("Average rate: " + (totalResolves.get() / Math.max(1, elapsed)) + "/s");
    System.out.println("==================");
  }
  
  // JUnit test entry point
  @org.junit.jupiter.api.Test
  void runLoadTestJUnit() throws Exception {
    runLoadTest(30); // Run for 30 seconds in test mode
  }
}
