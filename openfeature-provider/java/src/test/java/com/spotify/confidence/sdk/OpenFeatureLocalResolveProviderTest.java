package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.util.Timestamps;
import com.spotify.confidence.flags.resolver.v1.AppliedFlag;
import com.spotify.confidence.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.resolver.v1.events.FlagAssigned;
import com.spotify.confidence.flags.wasm.v1.WriteFlagLogsRequest;
import com.spotify.confidence.sdk.MaterializationStore.ReadOp;
import com.spotify.confidence.sdk.MaterializationStore.ReadResult;
import com.spotify.confidence.sdk.MaterializationStore.WriteOp;
import com.spotify.confidence.sdk.internal.engine.ExposureMode;
import com.spotify.confidence.sdk.internal.testutil.TestBackend;
import dev.openfeature.sdk.ErrorCode;
import dev.openfeature.sdk.ImmutableContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.MutableStructure;
import dev.openfeature.sdk.ProviderEvaluation;
import dev.openfeature.sdk.ProviderState;
import dev.openfeature.sdk.Value;
import dev.openfeature.sdk.exceptions.GeneralError;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.function.Consumer;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/**
 * The provider against the real module, as the Go provider's provider_test.go; that file is the
 * reference for the assertions. The basic.wasm fixture resolves flags/plain to the "on" variant
 * ({@code enabled=true, message="hello", count=42, ratio=0.5, nested={label:"inner"}}) and needs a
 * materialization store for flags/sticky (variant "sticky" = 99.99).
 */
class OpenFeatureLocalResolveProviderTest {

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

  /** A started provider with a published module. */
  private record Started(OpenFeatureLocalResolveProvider provider, TestBackend backend) {
    static Started of(
        OpenFeatureLocalResolveProviderTest suite, Consumer<LocalProviderConfig.Builder> mod) {
      TestBackend backend = new TestBackend();
      suite.cleanup.add(backend);
      backend.publish(TestBackend.module("basic.wasm"), "log-token", "\"v1\"");
      OpenFeatureLocalResolveProvider provider = suite.newProvider(backend, mod);
      provider.initialize(new MutableContext());
      return new Started(provider, backend);
    }
  }

  private Started started(Consumer<LocalProviderConfig.Builder> mod) {
    return Started.of(this, mod);
  }

  private OpenFeatureLocalResolveProvider newProvider(
      TestBackend backend, Consumer<LocalProviderConfig.Builder> mod) {
    LocalProviderConfig.Builder cfg =
        LocalProviderConfig.builder()
            .httpClientFactory(() -> backend.client())
            .cdnBaseUrl(backend.cdnUrl())
            .ingestBaseUrl(backend.ingestUrl())
            .retryInterval(Duration.ofMillis(5))
            .statePollInterval(Duration.ofMillis(10))
            .initializationTimeout(Duration.ofSeconds(5))
            .instances(2);
    if (mod != null) {
      mod.accept(cfg);
    }
    OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(cfg.build(), TestBackend.SECRET, backend.hexKey());
    cleanup.add(provider);
    return provider;
  }

  private static final ImmutableContext USER = new ImmutableContext("user-1");

  private static MutableStructure plainStructure() {
    MutableStructure nested = new MutableStructure(Map.of("label", new Value("")));
    return new MutableStructure(
        Map.of(
            "enabled", new Value(false),
            "message", new Value(""),
            "count", new Value(0),
            "ratio", new Value(0.0),
            "nested", new Value(nested)));
  }

  private static int assigned(TestBackend backend) {
    int n = 0;
    for (WriteFlagLogsRequest req : backend.logs()) {
      for (FlagAssigned fa : req.getFlagAssignedList()) {
        n += fa.getFlagsCount();
      }
    }
    return n;
  }

  // ------------------------------------------------------------------ evaluations

  @Test
  void testEvaluations() {
    OpenFeatureLocalResolveProvider p = started(null).provider();

    ProviderEvaluation<Boolean> bool = p.getBooleanEvaluation("plain.enabled", false, USER);
    assertTrue(bool.getValue());
    assertEquals("flags/plain/variants/on", bool.getVariant());
    assertEquals("TARGETING_MATCH", bool.getReason());
    assertEquals("confidence-sdk-java-local", p.getMetadata().getName());

    assertEquals("hello", p.getStringEvaluation("plain.message", "", USER).getValue());
    assertEquals(42, p.getIntegerEvaluation("plain.count", 0, USER).getValue());
    assertEquals(0.5, p.getDoubleEvaluation("plain.ratio", 0.0, USER).getValue());
    assertEquals("inner", p.getStringEvaluation("plain.nested.label", "", USER).getValue());

    ProviderEvaluation<Value> object =
        p.getObjectEvaluation("plain", new Value(plainStructure()), USER);
    assertEquals("flags/plain/variants/on", object.getVariant());
    assertEquals("TARGETING_MATCH", object.getReason());
    Value resolved = object.getValue();
    assertTrue(resolved.asStructure().getValue("enabled").asBoolean());
    assertEquals("hello", resolved.asStructure().getValue("message").asString());
    assertEquals(42, resolved.asStructure().getValue("count").asInteger());
    assertEquals(0.5, resolved.asStructure().getValue("ratio").asDouble());
    assertEquals(
        "inner",
        resolved.asStructure().getValue("nested").asStructure().getValue("label").asString());
  }

  @Test
  void testEvaluationErrors() {
    OpenFeatureLocalResolveProvider p = started(null).provider();

    // Wrong scalar types: the default comes back with TYPE_MISMATCH.
    ProviderEvaluation<Boolean> bool = p.getBooleanEvaluation("plain.message", true, USER);
    assertEquals(true, bool.getValue(), "the default is returned on error");
    assertEquals(ErrorCode.TYPE_MISMATCH, bool.getErrorCode());
    assertEquals("ERROR", bool.getReason());

    // A double where an int default was given.
    ProviderEvaluation<Integer> ratio = p.getIntegerEvaluation("plain.ratio", 7, USER);
    assertEquals(7, ratio.getValue());
    assertEquals(ErrorCode.TYPE_MISMATCH, ratio.getErrorCode());

    // A string where an int default was given under the structure.
    MutableStructure nestedInt = new MutableStructure(Map.of("label", new Value(0)));
    ProviderEvaluation<Value> nested =
        p.getObjectEvaluation("plain.nested", new Value(nestedInt), USER);
    assertEquals(ErrorCode.TYPE_MISMATCH, nested.getErrorCode());

    // Sticky flags need a materialization store.
    ProviderEvaluation<Double> sticky = p.getDoubleEvaluation("sticky.value", -1.0, USER);
    assertEquals(-1.0, sticky.getValue());
    assertEquals(ErrorCode.GENERAL, sticky.getErrorCode());

    // A non-existent flag.
    ProviderEvaluation<Boolean> missing = p.getBooleanEvaluation("nope.enabled", false, USER);
    assertEquals(false, missing.getValue());
    assertEquals(ErrorCode.FLAG_NOT_FOUND, missing.getErrorCode());
  }

  @Test
  void testNotReadyBeforeInit() {
    TestBackend backend = new TestBackend();
    cleanup.add(backend);
    backend.publish(TestBackend.module("basic.wasm"), "log-token", "\"v1\"");
    OpenFeatureLocalResolveProvider p = newProvider(backend, null);

    assertEquals(ProviderState.NOT_READY, p.getState());
    ProviderEvaluation<Boolean> d = p.getBooleanEvaluation("plain.enabled", false, USER);
    assertEquals(false, d.getValue());
    assertEquals(ErrorCode.PROVIDER_NOT_READY, d.getErrorCode());
    assertEquals("ERROR", d.getReason());
  }

  // --------------------------------------------------------------- materialization

  /** Answers every variant read with the sticky variant, as the Go stickyStore. */
  static final class StickyStore implements MaterializationStore {
    final List<WriteOp> writes = new ArrayList<>();

    @Override
    public CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops) {
      List<ReadResult> out = new ArrayList<>();
      for (ReadOp op : ops) {
        if (op instanceof ReadOp.Variant v) {
          out.add(v.toResult(Optional.of("flags/sticky/variants/sticky")));
        }
      }
      return CompletableFuture.completedFuture(out);
    }

    @Override
    public CompletionStage<Void> write(Set<? extends WriteOp> ops) {
      writes.addAll(ops);
      return CompletableFuture.completedFuture(null);
    }
  }

  @Test
  void testMaterializationStore() {
    StickyStore store = new StickyStore();
    OpenFeatureLocalResolveProvider p = started(c -> c.materializationStore(store)).provider();

    ProviderEvaluation<Double> d = p.getDoubleEvaluation("sticky.value", -1.0, USER);
    assertEquals(99.99, d.getValue());
    assertEquals("flags/sticky/variants/sticky", d.getVariant());
  }

  // ---------------------------------------------------------------- deferred apply

  @Test
  void testDeferredApply() {
    Started started = started(null);

    ResolveFlagsResponse response =
        started.provider().resolve(USER, List.of("plain"), false).toCompletableFuture().join();
    assertFalse(response.getResolveToken().isEmpty());
    assertEquals(1, response.getResolvedFlagsCount());
    assertEquals("flags/plain/variants/on", response.getResolvedFlags(0).getVariant());

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
    assertEquals(1, assigned(started.backend()));
  }

  // -------------------------------------------------------------- exposure modes

  @Test
  void testExposureModes() {
    for (Object[] row :
        new Object[][] {
          {ExposureMode.DEDUPLICATED, 1},
          {ExposureMode.ALL, 3},
          {ExposureMode.NONE, 0},
        }) {
      ExposureMode mode = (ExposureMode) row[0];
      int want = (Integer) row[1];
      Started started = started(c -> c.exposureMode(mode));
      for (int i = 0; i < 3; i++) {
        started.provider().getBooleanEvaluation("plain.enabled", false, USER);
      }
      started.provider().shutdown();
      assertEquals(want, assigned(started.backend()), "mode " + mode);
      for (TestBackend.Batch batch : started.backend().batches()) {
        assertEquals("log-token", batch.token(), "mode " + mode);
      }
    }
  }

  // ------------------------------------------------------------------- skip apply

  @Test
  void testSkipApply() {
    Started started = started(c -> c.exposureMode(ExposureMode.ALL));

    for (Object skip : new Object[] {Boolean.TRUE, "true", Boolean.FALSE}) {
      Value skipValue = skip instanceof Boolean b ? new Value(b) : new Value((String) skip);
      MutableContext ctx =
          new MutableContext("user-1", Map.of("_confidence_skip_apply", skipValue));
      ProviderEvaluation<Boolean> d =
          started.provider().getBooleanEvaluation("plain.enabled", false, ctx);
      assertEquals(null, d.getErrorCode());
    }

    started.provider().shutdown();
    assertEquals(2, assigned(started.backend()), "only the evaluations not skipped apply");
    int resolves = 0;
    for (WriteFlagLogsRequest req : started.backend().logs()) {
      for (var info : req.getClientResolveInfoList()) {
        for (var instance : info.getSchemaList()) {
          for (String key : instance.getSchemaMap().keySet()) {
            resolves++;
            assertNotEquals("_confidence_skip_apply", key, "skip-apply reached the module");
          }
        }
      }
    }
    assertTrue(resolves > 0, "no resolve info logged");
  }

  // --------------------------------------------------------------------- metrics

  @Test
  void testMetrics() {
    OpenFeatureLocalResolveProvider p = started(null).provider();
    p.getBooleanEvaluation("plain.enabled", false, USER);

    assertTrue(p.getPrometheusMetrics(new SnapshotConfig()).contains("resolve"));
    String openMetrics = p.getPrometheusMetrics(new SnapshotConfig(true));
    assertTrue(openMetrics.strip().endsWith("# EOF"), openMetrics);
  }

  // -------------------------------------------------------------------- shutdown

  @Test
  void testShutdownIsIdempotent() {
    OpenFeatureLocalResolveProvider p = started(null).provider();
    p.shutdown();
    assertEquals(ProviderState.NOT_READY, p.getState());
    assertDoesNotThrow(p::shutdown);
    assertDoesNotThrow(p::shutdown);
    assertEquals(ProviderState.NOT_READY, p.getState());
  }

  // ------------------------------------------------------------ config validation

  @Test
  void testConfigValidation() {
    String goodKey = "ab".repeat(32);
    TestBackend backend = new TestBackend();
    cleanup.add(backend);

    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider(null, goodKey));
    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider("", goodKey));
    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider("s", null));
    assertThrows(
        IllegalArgumentException.class,
        () -> new OpenFeatureLocalResolveProvider("s", goodKey.substring(2)));
    assertThrows(
        IllegalArgumentException.class,
        () -> new OpenFeatureLocalResolveProvider("s", "zz".repeat(32)));
    assertThrows(
        IllegalArgumentException.class,
        () -> new OpenFeatureLocalResolveProvider("s", goodKey + "\n"));

    // The store constructor argument and the remote store are mutually exclusive.
    assertThrows(
        IllegalArgumentException.class,
        () ->
            new OpenFeatureLocalResolveProvider(
                LocalProviderConfig.builder().useRemoteMaterializationStore(true).build(),
                "s",
                goodKey,
                new StickyStore()));
    assertThrows(
        IllegalArgumentException.class,
        () ->
            new OpenFeatureLocalResolveProvider(
                LocalProviderConfig.builder().materializationStore(new StickyStore()).build(),
                "s",
                goodKey,
                new StickyStore()));

    // Error messages must not leak the key.
    IllegalArgumentException e =
        assertThrows(
            IllegalArgumentException.class,
            () -> new OpenFeatureLocalResolveProvider("s", goodKey + "!"));
    assertTrue(e.getMessage().contains("64 hexadecimal"));
    assertFalse(e.getMessage().contains(goodKey));
  }

  // -------------------------------------------------------- startup and recovery

  @Test
  void testStartupTimeoutThenReadyInBackground() {
    TestBackend backend = new TestBackend();
    cleanup.add(backend);
    backend.setStatus(404);
    OpenFeatureLocalResolveProvider p =
        newProvider(
            backend,
            c ->
                c.initializationTimeout(Duration.ofMillis(200))
                    .statePollInterval(Duration.ofMillis(10)));

    assertEquals(ProviderState.NOT_READY, p.getState());
    GeneralError error = assertThrows(GeneralError.class, () -> p.initialize(new MutableContext()));
    assertNotNull(error.getMessage());
    assertEquals(ProviderState.NOT_READY, p.getState());

    // The engine keeps polling; the provider becomes READY when the module lands.
    backend.publish(TestBackend.module("basic.wasm"), "log-token", "\"v1\"");
    awaitState(p, ProviderState.READY);
    assertEquals(1, backend.registrations());
    assertTrue(p.getBooleanEvaluation("plain.enabled", false, USER).getValue());
  }

  @Test
  void testTerminalStartupRejected() {
    TestBackend backend = new TestBackend();
    cleanup.add(backend);
    backend.setStatus(403); // a 4xx other than 404 is terminal
    OpenFeatureLocalResolveProvider p = newProvider(backend, null);

    assertThrows(GeneralError.class, () -> p.initialize(new MutableContext()));
    assertEquals(ProviderState.FATAL, p.getState());
  }

  private static void awaitState(OpenFeatureLocalResolveProvider p, ProviderState want) {
    long deadline = System.nanoTime() + Duration.ofSeconds(5).toNanos();
    while (p.getState() != want) {
      if (System.nanoTime() > deadline) {
        throw new AssertionError("provider never reached " + want + "; at " + p.getState());
      }
      try {
        Thread.sleep(10);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw new AssertionError("interrupted");
      }
    }
  }
}
