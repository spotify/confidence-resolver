package com.spotify.confidence.sdk.internal.host;

import static com.spotify.confidence.sdk.internal.host.HostTestSupport.STICKY_RECORD;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.evaluate;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.newResolver;
import static com.spotify.confidence.sdk.internal.host.HostTestSupport.request;
import static org.assertj.core.api.Assertions.assertThat;

import com.google.protobuf.Value;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import java.util.List;
import org.junit.jupiter.api.Test;

/**
 * The Go host's BenchmarkEvaluate, as a plain timing report: one plain evaluate and one that
 * suspends for a materialization, on the runtime-compiler engine. Asserts correctness; prints
 * throughput.
 */
class HostBenchmarkTest {

  @Test
  void reportEvaluateThroughput() {
    record Case(String name, String flag, String key, boolean sticky) {}
    List<Case> cases =
        List.of(
            new Case("plain", "flags/plain", "plain.enabled", false),
            new Case("sticky-suspend", "flags/sticky", "sticky.value", true));
    for (Case c : cases) {
      int warmup = 500;
      int iterations = 10_000;
      try (Resolver r = newResolver(false, new HostConfig.Builder().instances(4))) {
        EvaluateRequest eval =
            EvaluateRequest.newBuilder()
                .setFlagKey(c.key())
                .setDefaultValue(Value.newBuilder().setBoolValue(false).build())
                .build();
        HostTestSupport.FakeStore store =
            new HostTestSupport.FakeStore(c.sticky() ? STICKY_RECORD : null);
        for (int i = 0; i < warmup; i++) {
          evaluate(r, request("u" + i, c.flag()), store, eval);
        }
        long started = System.nanoTime();
        ResolutionDetails last = null;
        for (int i = 0; i < iterations; i++) {
          last = evaluate(r, request("u" + i, c.flag()), store, eval).get(0);
        }
        long nanos = System.nanoTime() - started;
        assertThat(last).isNotNull();
        System.out.printf(
            "  host %-15s %8.1f us/evaluate%n", c.name(), nanos / 1_000.0 / iterations);
      }
    }
  }
}
