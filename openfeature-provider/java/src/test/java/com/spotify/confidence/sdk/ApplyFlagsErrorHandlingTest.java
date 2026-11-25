package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.util.Timestamps;
import com.spotify.confidence.flags.resolver.v1.AppliedFlag;
import com.spotify.confidence.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.resolver.v1.events.FlagAssigned;
import com.spotify.confidence.flags.wasm.v1.WriteFlagLogsRequest;
import com.spotify.confidence.sdk.internal.engine.ExposureMode;
import com.spotify.confidence.sdk.internal.testutil.TestBackend;
import dev.openfeature.sdk.ImmutableContext;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/**
 * Applies are best-effort telemetry, as the old stack's ApplyFlagsErrorHandlingTest; that file is
 * the reference for the swallow-and-log semantics. Malformed and rejected applies never throw and
 * never log an assignment.
 */
class ApplyFlagsErrorHandlingTest {

  private final List<Object> cleanup = new ArrayList<>();

  @AfterEach
  void tearDown() {
    for (int i = cleanup.size() - 1; i >= 0; i--) {
      Object c = cleanup.get(i);
      try {
        if (c instanceof OpenFeatureLocalResolveProvider p) {
          p.shutdown();
        } else if (c instanceof AutoCloseable a) {
          a.close();
        }
      } catch (Exception ignored) {
        // cleanup only
      }
    }
  }

  private static final ImmutableContext USER = new ImmutableContext("user-1");

  private record Started(OpenFeatureLocalResolveProvider provider, TestBackend backend) {}

  private Started started(ExposureMode mode) {
    TestBackend backend = new TestBackend();
    cleanup.add(backend);
    backend.publish(TestBackend.module("basic.wasm"), "log-token", "\"v1\"");
    OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(
            LocalProviderConfig.builder()
                .httpClientFactory(() -> backend.client())
                .cdnBaseUrl(backend.cdnUrl())
                .resolverBaseUrl(backend.resolverUrl())
                .ingestBaseUrl(backend.ingestUrl())
                .retryInterval(Duration.ofMillis(5))
                .statePollInterval(Duration.ofMillis(10))
                .initializationTimeout(Duration.ofSeconds(5))
                .exposureMode(mode)
                .instances(2)
                .build(),
            TestBackend.SECRET,
            backend.hexKey());
    cleanup.add(provider);
    provider.initialize(new ImmutableContext());
    return new Started(provider, backend);
  }

  private List<FlagAssigned.AppliedFlag> appliedFlags(TestBackend backend) {
    List<FlagAssigned.AppliedFlag> out = new ArrayList<>();
    for (WriteFlagLogsRequest req : backend.logs()) {
      for (FlagAssigned assigned : req.getFlagAssignedList()) {
        out.addAll(assigned.getFlagsList());
      }
    }
    return out;
  }

  @Test
  void applyWithoutSendTime_isAcceptedByTheModule() {
    // Unlike the old stack's module, the state-to-wasm module carries the apply through even
    // without send_time; the old swallow-and-log case no longer exists.
    Started started = started(ExposureMode.DEDUPLICATED);
    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), false).toCompletableFuture().join();

    assertDoesNotThrow(
        () ->
            started
                .provider()
                .applyFlags(
                    ApplyFlagsRequest.newBuilder()
                        .setResolveToken(response.getResolveToken())
                        .addFlags(
                            AppliedFlag.newBuilder()
                                .setFlag("flags/plain")
                                .setApplyTime(Timestamps.now()))
                        .build()));

    started.provider().shutdown();
    assertEquals(1, appliedFlags(started.backend()).size());
  }

  @Test
  void applyWithSendTime_recordsOneFlagAssigned() {
    Started started = started(ExposureMode.DEDUPLICATED);
    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), false).toCompletableFuture().join();

    started
        .provider()
        .applyFlags(
            ApplyFlagsRequest.newBuilder()
                .setResolveToken(response.getResolveToken())
                .setSendTime(Timestamps.now())
                .addFlags(
                    AppliedFlag.newBuilder().setFlag("flags/plain").setApplyTime(Timestamps.now()))
                .build());

    started.provider().shutdown();
    List<FlagAssigned.AppliedFlag> applied = appliedFlags(started.backend());
    assertEquals(1, applied.size());
    FlagAssigned.AppliedFlag flag = applied.get(0);
    assertEquals("flags/plain", flag.getFlag());
    assertEquals("user-1", flag.getTargetingKey());
    assertTrue(flag.hasAssignmentInfo(), "expected a concrete assignment");
    assertEquals("flags/plain/variants/on", flag.getAssignmentInfo().getVariant());
    assertNotNull(flag.getApplyTime(), "apply time is required");
    assertTrue(Timestamps.toMillis(flag.getApplyTime()) > 0);
  }

  @Test
  void applyAfterSecretRotated_isSwallowedAndLogged() {
    Started started = started(ExposureMode.DEDUPLICATED);
    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), false).toCompletableFuture().join();

    // The request carries a stale secret; the module rejects it and the apply is swallowed.
    assertDoesNotThrow(
        () ->
            started
                .provider()
                .applyFlags(
                    ApplyFlagsRequest.newBuilder()
                        .setClientSecret("another-secret")
                        .setResolveToken(response.getResolveToken())
                        .setSendTime(Timestamps.now())
                        .addFlags(
                            AppliedFlag.newBuilder()
                                .setFlag("flags/plain")
                                .setApplyTime(Timestamps.now()))
                        .build()));

    started.provider().shutdown();
    assertEquals(0, appliedFlags(started.backend()).size());
  }

  @Test
  void resolveWithApplyTrueAndNoExposure_flushesResolveInfosNotAssigns() {
    Started started = started(ExposureMode.NONE);

    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), true).toCompletableFuture().join();
    assertTrue(response.getResolveToken().isEmpty(), "apply=true returns no token");

    started.provider().shutdown();
    assertEquals(0, appliedFlags(started.backend()).size(), "no exposures in NONE mode");
    int clientInfos = 0;
    int flagInfos = 0;
    for (WriteFlagLogsRequest req : started.backend().logs()) {
      clientInfos += req.getClientResolveInfoCount();
      flagInfos += req.getFlagResolveInfoCount();
    }
    assertTrue(clientInfos >= 1, "client resolve info expected");
    assertTrue(flagInfos >= 1, "flag resolve info expected");
  }

  @Test
  void applyWithEmptyFlags_isNoOp() {
    Started started = started(ExposureMode.DEDUPLICATED);
    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), false).toCompletableFuture().join();

    assertDoesNotThrow(
        () ->
            started
                .provider()
                .applyFlags(
                    ApplyFlagsRequest.newBuilder()
                        .setResolveToken(response.getResolveToken())
                        .setSendTime(Timestamps.now())
                        .build()));

    started.provider().shutdown();
    assertEquals(0, appliedFlags(started.backend()).size());
  }
}
