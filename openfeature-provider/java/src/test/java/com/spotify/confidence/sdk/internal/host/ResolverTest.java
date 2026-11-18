package com.spotify.confidence.sdk.internal.host;

import static com.spotify.confidence.sdk.internal.host.HostTestSupport.STICKY_RECORD;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.bothEngines;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.evaluate;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.newResolver;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.request;
import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.spotify.confidence.flags.resolver.v1.ResolveReason;
import com.spotify.confidence.flags.types.v1.FlagSchema;
import com.spotify.confidence.flags.wasm.v1.ErrorCode;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

/** Ports the Go host's resolver_test.go. */
class ResolverTest {

  @Test
  void evaluatesEveryFlagTypeAgainstAResolve() {
    bothEngines(
        interp -> {
          try (Resolver r = newResolver(interp, new HostConfig.Builder())) {
            List<ResolutionDetails> d =
                evaluate(
                    r,
                    request("u1", "flags/plain"),
                    new HostTestSupport.FakeStore(),
                    EvaluateRequest.newBuilder()
                        .setFlagKey("plain")
                        .setDefaultValue(
                            struct(
                                "enabled", Value.newBuilder().setBoolValue(false),
                                "message", Value.newBuilder().setStringValue("default")))
                        .build(),
                    EvaluateRequest.newBuilder()
                        .setFlagKey("plain.nested.label")
                        .setDefaultValue(Value.newBuilder().setStringValue("x").build())
                        .build(),
                    EvaluateRequest.newBuilder()
                        .setFlagKey("plain.count")
                        .setDefaultValue(Value.newBuilder().setNumberValue(0).build())
                        .setExpectedSchema(
                            FlagSchema.newBuilder()
                                .setIntSchema(FlagSchema.IntFlagSchema.getDefaultInstance())
                                .build())
                        .build(),
                    EvaluateRequest.newBuilder()
                        .setFlagKey("plain.message")
                        .setDefaultValue(Value.newBuilder().setBoolValue(true).build())
                        .build());
            Struct whole = d.get(0).getValue().getStructValue();
            assertThat(whole.getFieldsOrThrow("enabled").getBoolValue()).isTrue();
            assertThat(whole.getFieldsOrThrow("message").getStringValue()).isEqualTo("hello");
            assertThat(whole.getFieldsOrThrow("count").getNumberValue()).isEqualTo(42);
            assertThat(d.get(0).getVariant()).isEqualTo("flags/plain/variants/on");
            assertThat(d.get(0).getReason()).isEqualTo(ResolveReason.RESOLVE_REASON_MATCH);
            assertThat(d.get(1).getValue().getStringValue()).isEqualTo("inner");
            assertThat(d.get(2).getValue().getNumberValue()).isEqualTo(42);
            assertThat(d.get(3).getErrorCode()).isEqualTo(ErrorCode.ERROR_CODE_TYPE_MISMATCH);
            assertThat(d.get(3).getValue().getBoolValue()).isTrue(); // the default is kept
          }
        });
  }

  @Test
  void stickyFlagsSuspendAndResumeThroughTheStore() {
    bothEngines(
        interp -> {
          try (Resolver r = newResolver(interp, new HostConfig.Builder())) {
            EvaluateRequest eval =
                EvaluateRequest.newBuilder()
                    .setFlagKey("sticky.value")
                    .setDefaultValue(Value.newBuilder().setNumberValue(-1).build())
                    .build();
            HostTestSupport.FakeStore withRecord = new HostTestSupport.FakeStore(STICKY_RECORD);
            ResolutionDetails sticky =
                evaluate(r, request("u1", "flags/sticky"), withRecord, eval).get(0);
            assertThat(sticky.getValue().getNumberValue()).isEqualTo(99.99);
            assertThat(sticky.getVariant()).isEqualTo("flags/sticky/variants/sticky");
            assertThat(withRecord.reads.get()).isEqualTo(1);

            HostTestSupport.FakeStore withoutRecord = new HostTestSupport.FakeStore();
            ResolutionDetails fresh =
                evaluate(r, request("u1", "flags/sticky"), withoutRecord, eval).get(0);
            assertThat(fresh.getValue().getNumberValue()).isEqualTo(1.0);
            assertThat(fresh.getVariant()).isEqualTo("flags/sticky/variants/fresh");
            assertThat(withoutRecord.reads.get()).isEqualTo(1);
          }
        });
  }

  @Test
  void rejectsAnUnknownClient() {
    try (Resolver r = newResolver(false, new HostConfig.Builder())) {
      assertThatThrownBy(
              () ->
                  r.evaluate(
                      request("u1", "flags/plain").toBuilder().setClientSecret("nope").build(),
                      List.of(),
                      new HostTestSupport.FakeStore()))
          .isInstanceOf(ResolveRejectException.class)
          .hasMessageContaining("UNAUTHENTICATED");
    }
  }

  @Test
  void concurrentResolvesKeepLogsComingAndCloseFlushes() {
    bothEngines(
        interp -> {
          HostTestSupport.LogSink sink = new HostTestSupport.LogSink();
          try (Resolver r =
              newResolver(interp, new HostConfig.Builder().instances(4).writeLogs(sink::write))) {
            int threads = 8;
            int each = 200;
            EvaluateRequest eval =
                EvaluateRequest.newBuilder()
                    .setFlagKey("plain.enabled")
                    .setDefaultValue(Value.newBuilder().setBoolValue(false).build())
                    .build();
            var failures = new AtomicInteger();
            var threadList = new CopyOnWriteArrayList<Thread>();
            for (int g = 0; g < threads; g++) {
              final int gi = g;
              Thread t =
                  new Thread(
                      () -> {
                        for (int i = 0; i < each; i++) {
                          try {
                            var d =
                                evaluate(
                                    r,
                                    request("u-" + gi + "-" + i, "flags/plain"),
                                    new HostTestSupport.FakeStore(),
                                    eval);
                            if (!d.get(0).getValue().getBoolValue()) {
                              failures.incrementAndGet();
                            }
                          } catch (RuntimeException e) {
                            failures.incrementAndGet();
                          }
                        }
                      });
              threadList.add(t);
              t.start();
            }
            for (Thread t : threadList) {
              try {
                t.join();
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
            }
            assertThat(failures.get()).isZero();
            r.flushLogs();
            sink.assertThatLogsAreWellFormed();
            assertThat(sink.assigned()).isEqualTo(threads * each);
            assertThat(sink.batches).isNotEmpty();

            evaluate(r, request("u1", "flags/plain"), new HostTestSupport.FakeStore());
          }
          // The resolver already closed (try-with-resources): it flushed the last evaluate.
          assertThat(sink.assigned()).isEqualTo(8 * 200 + 1);
        });
  }

  @Test
  void aCapBelowTheModuleMinimumIsRejected() {
    assertThatThrownBy(
            () ->
                new Resolver(
                    Module.parse(HostTestSupport.fixture()),
                    new HostConfig.Builder().maxMemoryBytes(1).build()))
        .isInstanceOf(InvalidModuleException.class)
        .hasMessageContaining("below the module's minimum");
  }

  @Test
  void anOverTightCapFailsAtInstanceBuildNotMidResolve() {
    // basic.wasm needs more than two pages at init; the module traps while instantiating, so
    // the bad cap surfaces at the first instance, mapped to InvalidModule.
    assertThatThrownBy(
            () -> {
              Resolver r =
                  new Resolver(
                      Module.parse(HostTestSupport.fixture()),
                      new HostConfig.Builder().maxMemoryBytes(2L * 64 * 1024).build());
              try {
                r.evaluate(
                    request("u1", "flags/plain"), List.of(), new HostTestSupport.FakeStore());
              } finally {
                r.close();
              }
            })
        .isInstanceOf(InvalidModuleException.class);
  }

  @Test
  void theDedupeFilterMemoryOutlivesResolvers() {
    DedupeFilter filter = new DedupeFilter(8);
    // Two generations live at once, as during a swap; then the first retires.
    java.util.function.Function<HostTestSupport.LogSink, Resolver> newGen =
        sink -> {
          Module module = Module.parse(HostTestSupport.fixture());
          return new Resolver(
              module, new HostConfig.Builder().writeLogs(sink::write).applyFilter(filter).build());
        };
    HostTestSupport.LogSink oldSink = new HostTestSupport.LogSink();
    HostTestSupport.LogSink curSink = new HostTestSupport.LogSink();
    try (Resolver old = newGen.apply(oldSink);
        Resolver cur = newGen.apply(curSink)) {
      assertThat(assignedBy(old, "u1", oldSink)).isEqualTo(1);
      assertThat(assignedBy(cur, "u1", curSink)).isZero(); // same filter memory
    }
    HostTestSupport.LogSink afterSink = new HostTestSupport.LogSink();
    try (Resolver after = newGen.apply(afterSink)) {
      assertThat(assignedBy(after, "u1", afterSink)).isZero(); // history survived the swap
      assertThat(assignedBy(after, "u2", afterSink)).isEqualTo(1);
    }
  }

  private int assignedBy(Resolver r, String unit, HostTestSupport.LogSink sink) {
    int before = sink.assigned();
    evaluate(r, request(unit, "flags/plain"), new HostTestSupport.FakeStore());
    r.flushLogs();
    return sink.assigned() - before;
  }

  @Test
  void resolveReturnsTheResponseTokenAndMetrics() {
    try (Resolver r = newResolver(false, new HostConfig.Builder())) {
      var response =
          r.resolve(
              request("u1", "flags/plain").toBuilder().setApply(false).build(),
              new HostTestSupport.FakeStore());
      assertThat(response.getResolvedFlagsCount()).isEqualTo(1);
      assertThat(response.getResolvedFlags(0).getVariant()).isEqualTo("flags/plain/variants/on");
      assertThat(response.getResolveToken()).isNotEmpty();
      for (boolean openMetrics : new boolean[] {false, true}) {
        assertThat(r.metrics(openMetrics)).contains("resolve");
      }
      assertThat(r.metrics(false)).contains("latency");
    }
  }

  @Test
  void anAbandonedSuspendedProcessIsDiscardedWithoutLeaking() {
    // The 1 MiB cap leaves the module running but cannot hold the 4 MiB resume argument:
    // the allocation must fail, surface as an error, and discard the suspended process.
    try (Resolver r =
        new Resolver(
            Module.parse(HostTestSupport.fixture()),
            new HostConfig.Builder().maxMemoryBytes(1L << 20).build())) {
      EvaluateRequest eval =
          EvaluateRequest.newBuilder()
              .setFlagKey("sticky.value")
              .setDefaultValue(Value.newBuilder().setNumberValue(-1).build())
              .build();
      evaluate(r, request("u1", "flags/sticky"), new HostTestSupport.FakeStore(), eval);
      long before = r.allocatedBytes();
      int discards = r.discardCalls();

      Record huge = STICKY_RECORD.toBuilder().setVariant("x".repeat(4 << 20)).build();
      assertThatThrownBy(
              () ->
                  evaluate(
                      r, request("u1", "flags/sticky"), new HostTestSupport.FakeStore(huge), eval))
          .isInstanceOf(AllocationException.class);
      assertThat(r.allocatedBytes()).isEqualTo(before); // nothing leaked
      assertThat(r.discardCalls() - discards).isEqualTo(1);

      ResolutionDetails sticky =
          evaluate(
                  r,
                  request("u1", "flags/sticky"),
                  new HostTestSupport.FakeStore(STICKY_RECORD),
                  eval)
              .get(0);
      assertThat(sticky.getVariant()).isEqualTo("flags/sticky/variants/sticky");
    }
  }

  @Test
  void callsAfterCloseFailWithClosed() {
    try (Resolver r = newResolver(false, new HostConfig.Builder())) {
      r.close();
      r.close(); // idempotent
      assertThatThrownBy(
              () -> evaluate(r, request("u1", "flags/plain"), new HostTestSupport.FakeStore()))
          .isInstanceOf(ClosedException.class);
    }
  }

  private static Value struct(Object... pairs) {
    Struct.Builder builder = Struct.newBuilder();
    for (int i = 0; i < pairs.length; i += 2) {
      builder.putFields((String) pairs[i], ((Value.Builder) pairs[i + 1]).build());
    }
    return Value.newBuilder().setStructValue(builder).build();
  }
}
