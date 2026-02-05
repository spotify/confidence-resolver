package com.spotify.confidence.sdk;

import com.google.protobuf.util.JsonFormat;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import org.junit.jupiter.api.Test;

/**
 * Debug test to print the WriteFlagLogsRequest payload.
 * 
 * Run with: mvn test -Dtest=DebugPayloadTest -DfailIfNoTests=false
 */
public class DebugPayloadTest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "debug-test-user";
  
  @Test
  void debugWriteFlagLogsPayload() throws Exception {
    System.out.println("=== DEBUG: WriteFlagLogsRequest Payload Test ===\n");
    
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "1");
    
    // Create capturing logger to intercept the payload
    CapturingWasmFlagLogger capturingLogger = new CapturingWasmFlagLogger();
    
    // Create state provider
    final var stateProvider =
        new FlagsAdminStateFetcher(FLAG_CLIENT_SECRET, new DefaultHttpClientFactory());
    System.out.println("Fetching flag state...");
    stateProvider.reload();
    System.out.println("Flag state loaded.\n");
    
    // Create provider with capturing logger
    final var provider =
        new OpenFeatureLocalResolveProvider(
            stateProvider,
            FLAG_CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            capturingLogger);
    
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
    
    // Set evaluation context
    final EvaluationContext context = new MutableContext(TARGETING_KEY).add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);
    
    final Client client = OpenFeatureAPI.getInstance().getClient();
    
    // Clear any logs from initialization
    capturingLogger.clear();
    
    System.out.println("=== Performing flag evaluations ===\n");
    
    // 1. Valid flag
    System.out.println("1. Resolving valid flag: web-sdk-e2e-flag.bool");
    boolean value1 = client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    System.out.println("   Result: " + value1 + "\n");
    
    // 2. Non-existent flag (should trigger FLAG_NOT_FOUND)
    System.out.println("2. Resolving non-existent flag: totally-fake-flag.bool");
    boolean value2 = client.getBooleanValue("totally-fake-flag.bool", false);
    System.out.println("   Result: " + value2 + " (expected default)\n");
    
    // 3. Another valid flag
    System.out.println("3. Resolving another valid flag: web-sdk-e2e-flag.bool");
    boolean value3 = client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    System.out.println("   Result: " + value3 + "\n");
    
    // Flush logs
    System.out.println("=== Flushing logs (triggering checkpoint) ===\n");
    provider.shutdown();
    
    // Print captured requests
    System.out.println("=== Captured WriteFlagLogsRequest payloads ===\n");
    
    var requests = capturingLogger.getCapturedRequests();
    System.out.println("Total captured requests: " + requests.size() + "\n");
    
    JsonFormat.Printer jsonPrinter = JsonFormat.printer().includingDefaultValueFields();
    
    for (int i = 0; i < requests.size(); i++) {
      WriteFlagLogsRequest req = requests.get(i);
      System.out.println("--- Request " + (i + 1) + " ---");
      System.out.println("Flag assigned count: " + req.getFlagAssignedCount());
      System.out.println("Client resolve info count: " + req.getClientResolveInfoCount());
      System.out.println("Flag resolve info count: " + req.getFlagResolveInfoCount());
      System.out.println("Has telemetry data: " + req.hasTelemetryData());
      
      if (req.hasTelemetryData()) {
        var telemetry = req.getTelemetryData();
        System.out.println("\n  Telemetry Data:");
        System.out.println("    Has SDK: " + telemetry.hasSdk());
        if (telemetry.hasSdk()) {
          var sdk = telemetry.getSdk();
          System.out.println("    SDK id case: " + sdk.getSdkCase());
          if (sdk.getSdkCase() == com.spotify.confidence.sdk.flags.resolver.v1.Sdk.SdkCase.ID) {
            System.out.println("    SDK id: " + sdk.getId());
          } else if (sdk.getSdkCase() == com.spotify.confidence.sdk.flags.resolver.v1.Sdk.SdkCase.CUSTOM_ID) {
            System.out.println("    SDK custom_id: " + sdk.getCustomId());
          }
          System.out.println("    SDK version: " + sdk.getVersion());
        }
        
        System.out.println("    Resolve errors count: " + telemetry.getResolveErrorsCount());
        for (var error : telemetry.getResolveErrorsList()) {
          System.out.println("      - error_code: " + error.getErrorCode() + " (" + error.getErrorCode().name() + ")");
          System.out.println("        count: " + error.getCount());
        }
      }
      
      System.out.println("\n  Full JSON payload:");
      try {
        String json = jsonPrinter.print(req);
        System.out.println(json);
      } catch (Exception e) {
        System.out.println("  Error printing JSON: " + e.getMessage());
      }
      System.out.println();
    }
    
    System.out.println("=== End of debug output ===");
  }
}
