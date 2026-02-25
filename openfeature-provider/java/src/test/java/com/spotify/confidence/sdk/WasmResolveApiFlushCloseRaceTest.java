package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.concurrent.CountDownLatch;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Regression test for the race condition between {@link WasmResolveApi#flushAssignLogs()} and
 * {@link WasmResolveApi#close()}. Previously both methods acquired a readLock, allowing concurrent
 * execution on the same WASM instance. When they raced, flag assignments were lost — close() found
 * 0 entries because flushAssignLogs() already drained them (or vice versa, with corrupted WASM
 * memory access). The fix changes both methods to use writeLock for exclusive access.
 */
class WasmResolveApiFlushCloseRaceTest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "test-race";

  private static byte[] resolverState;
  private static String accountId;

  @BeforeAll
  static void fetchState() {
    final var stateProvider =
        new FlagsAdminStateFetcher(FLAG_CLIENT_SECRET, new DefaultHttpClientFactory());
    stateProvider.reload();
    resolverState = stateProvider.provide();
    accountId = stateProvider.accountId();
  }

  @Test
  void concurrentFlushAndCloseShouldNotLoseAssignments() throws Exception {
    final int iterations = 100;
    int lostAssigns = 0;

    for (int i = 0; i < iterations; i++) {
      final var logger = new CapturingWasmFlagLogger();
      final var api = new WasmResolveApi(logger);
      api.setResolverState(resolverState, accountId);

      // Resolve a flag to create a flag assignment in the WASM buffer
      api.resolveProcess(buildResolveRequest());

      // Race: flushAssignLogs and close concurrently
      final var startLatch = new CountDownLatch(1);
      final var doneLatch = new CountDownLatch(2);

      final Thread flusher =
          new Thread(
              () -> {
                try {
                  startLatch.await();
                  api.flushAssignLogs();
                } catch (InterruptedException e) {
                  Thread.currentThread().interrupt();
                } finally {
                  doneLatch.countDown();
                }
              });

      final Thread closer =
          new Thread(
              () -> {
                try {
                  startLatch.await();
                  api.close();
                } catch (Exception e) {
                  // close() may fail due to corrupted WASM state from concurrent access
                } finally {
                  doneLatch.countDown();
                }
              });

      flusher.start();
      closer.start();
      startLatch.countDown();
      doneLatch.await();

      // Count total flag assignments across all captured requests
      final int totalAssigns =
          logger.getCapturedRequests().stream()
              .mapToInt(WriteFlagLogsRequest::getFlagAssignedCount)
              .sum();

      if (totalAssigns == 0) {
        lostAssigns++;
      }
    }

    assertThat(lostAssigns)
        .as(
            "Flag assignments were lost in %d out of %d iterations due to concurrent "
                + "WASM memory access in flushAssignLogs() and close()",
            lostAssigns,
            iterations)
        .isEqualTo(0);
  }

  private static ResolveProcessRequest buildResolveRequest() {
    final var resolveFlagsRequest =
        ResolveFlagsRequest.newBuilder()
            .addFlags("flags/web-sdk-e2e-flag")
            .setApply(true)
            .setClientSecret(FLAG_CLIENT_SECRET)
            .setEvaluationContext(
                Struct.newBuilder()
                    .putFields(
                        "targeting_key",
                        Value.newBuilder().setStringValue(TARGETING_KEY).build())
                    .build())
            .setSdk(
                Sdk.newBuilder()
                    .setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER)
                    .setVersion(Version.VERSION)
                    .build())
            .build();

    return ResolveProcessRequest.newBuilder()
        .setWithoutMaterializations(resolveFlagsRequest)
        .build();
  }
}
