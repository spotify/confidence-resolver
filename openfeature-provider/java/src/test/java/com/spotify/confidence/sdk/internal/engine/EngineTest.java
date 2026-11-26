package com.spotify.confidence.sdk.internal.engine;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.sdk.internal.cdn.CdnFetcher;
import com.spotify.confidence.sdk.internal.host.MaterializationStore;
import com.spotify.confidence.sdk.internal.testutil.TestBackend;
import java.time.Duration;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;
import org.awaitility.Awaitility;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.slf4j.helpers.NOPLogger;

/** Ports the Go engine tests: startup, swap under load, broken modules, and close. */
class EngineTest {

  private TestBackend backend;
  private Engine engine;

  @AfterEach
  void stop() {
    if (engine != null) {
      engine.close(Duration.ofSeconds(5));
    }
    if (backend != null) {
      backend.close();
    }
  }

  private Engine newEngine(TestBackend b, Consumer<EngineConfig.Builder> mod) {
    EngineConfig.Builder cfg =
        new EngineConfig.Builder()
            .clientSecret(TestBackend.SECRET)
            .encryptionKey(CdnFetcher.parseKey(b.hexKey()))
            .cdnBaseUrl(b.cdnUrl())
            .ingestBaseUrl(b.ingestUrl())
            .instances(4)
            .pollInterval(Duration.ofMillis(10))
            .flushInterval(Duration.ofHours(1))
            .retryInterval(Duration.ofMillis(5))
            .logger(NOPLogger.NOP_LOGGER);
    if (mod != null) {
      mod.accept(cfg);
    }
    return new Engine(cfg.build());
  }

  /** The store the Go tests use: no records, no writes. */
  static final class NoStore implements MaterializationStore {

    @Override
    public List<Record> read(List<Record> requested) {
      return List.of();
    }
  }

  private static final List<EvaluateRequest> EVAL_ENABLED =
      List.of(
          EvaluateRequest.newBuilder()
              .setFlagKey("plain.enabled")
              .setDefaultValue(Value.newBuilder().setBoolValue(false).build())
              .build());

  private static ResolveFlagsRequest request(String unit, String flag) {
    return ResolveFlagsRequest.newBuilder()
        .addFlags(flag)
        .setEvaluationContext(
            Struct.newBuilder()
                .putFields("targeting_key", Value.newBuilder().setStringValue(unit).build()))
        .setClientSecret(TestBackend.SECRET)
        .setApply(true)
        .build();
  }

  private void evaluate(Engine e, String unit) {
    var details = e.evaluate(request(unit, "flags/plain"), EVAL_ENABLED, new NoStore());
    assertThat(details.get(0).getValue().getBoolValue()).isTrue();
  }

  private static Map<String, Integer> tokens(TestBackend b) {
    Map<String, Integer> tokens = new HashMap<>();
    for (TestBackend.Batch batch : b.batches()) {
      tokens.merge(batch.token(), 1, Integer::sum);
    }
    return tokens;
  }

  @Test
  void startsEvaluatesCloses() {
    backend = new TestBackend();
    backend.publish(TestBackend.module("basic.wasm"), "tok-1", "\"v1\"");
    engine = newEngine(backend, null);
    assertThatThrownBy(() -> evaluate(engine, "u")).isInstanceOf(NotReadyException.class);

    engine.start(Duration.ofSeconds(1));
    evaluate(engine, "u");
    assertThat(engine.close(Duration.ofSeconds(5))).isTrue();

    assertThat(tokens(backend)).containsEntry("tok-1", 1);
    assertThatThrownBy(() -> evaluate(engine, "u")).isInstanceOf(EngineClosedException.class);
  }

  @Test
  void swapsWithoutDroppingEvaluations() {
    backend = new TestBackend();
    byte[] wasm = TestBackend.module("basic.wasm");
    backend.publish(wasm, "tok-0", "\"v0\"");
    engine = newEngine(backend, null);
    engine.start(Duration.ofSeconds(1));

    AtomicBoolean stop = new AtomicBoolean();
    AtomicLong failures = new AtomicLong();
    AtomicLong done = new AtomicLong();
    var threads = new java.util.ArrayList<Thread>();
    for (int g = 0; g < 8; g++) {
      final int gi = g;
      Thread t =
          new Thread(
              () -> {
                for (int i = 0; !stop.get(); i++) {
                  try {
                    evaluate(engine, "u-" + gi + "-" + i);
                  } catch (RuntimeException e) {
                    failures.incrementAndGet();
                  }
                  done.incrementAndGet();
                }
              });
      threads.add(t);
      t.start();
    }
    int swaps = 5;
    for (int i = 1; i <= swaps; i++) {
      Engine.Generation before = engine.currentGeneration();
      String token = "tok-" + i;
      backend.publish(wasm, token, "\"v" + i + "\"");
      Engine.Generation beforeFinal = before;
      Awaitility.await()
          .atMost(5, TimeUnit.SECONDS)
          .until(() -> engine.currentGeneration() != beforeFinal);
    }
    stop.set(true);
    for (Thread t : threads) {
      try {
        t.join();
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
    }
    assertThat(engine.close(Duration.ofSeconds(5))).isTrue();
    assertThat(failures.get()).isZero();
    Map<String, Integer> got = tokens(backend);
    for (int i = 0; i <= swaps; i++) {
      assertThat(got.get("tok-" + i)).as("generation %d's token", i).isPositive();
    }
  }

  @Test
  void aBrokenModuleKeepsTheCurrentOneAndIsRefetchedInFull() {
    backend = new TestBackend();
    backend.publish(TestBackend.module("basic.wasm"), "tok-good", "\"good\"");
    engine = newEngine(backend, null);
    engine.start(Duration.ofSeconds(1));
    Engine.Generation good = engine.currentGeneration();

    backend.publish("not wasm".getBytes(), "tok-bad", "\"bad\"");
    Awaitility.await()
        .atMost(5, TimeUnit.SECONDS)
        .until(
            () -> {
              int conditional = 0;
              for (String inm : backend.ifNoneMatch()) {
                if (inm.equals("\"good\"")) {
                  conditional++;
                }
              }
              return conditional >= 3; // every poll is still conditional on the good ETag
            });
    assertThat(engine.currentGeneration()).isSameAs(good);
    evaluate(engine, "u");
  }

  @Test
  void closeDrainsAnInFlightPollInsteadOfInterruptingIt() {
    // Endive aborts a module call when its thread is interrupted, so shutdownNow() on the
    // engine's own loops could abort a poll's wasm call midway. close() drains instead: the
    // in-flight fetch finishes (bounded by its request timeout) and no poll failure is logged.
    org.slf4j.Logger logger = org.mockito.Mockito.mock(org.slf4j.Logger.class);
    backend = new TestBackend();
    backend.publish(TestBackend.module("basic.wasm"), "tok-1", "\"v1\"");
    engine = newEngine(backend, c -> c.pollInterval(Duration.ofMillis(20)).logger(logger));
    engine.start(Duration.ofSeconds(1));
    backend.delayCdn(400); // hold every subsequent fetch in flight server-side
    Awaitility.await()
        .atMost(5, TimeUnit.SECONDS)
        .until(() -> backend.ifNoneMatch().size() >= 2); // a poll fetch is in flight
    assertThat(engine.close(Duration.ofSeconds(5))).isTrue();
    org.mockito.Mockito.verify(logger, org.mockito.Mockito.never())
        .warn(
            org.mockito.ArgumentMatchers.anyString(),
            org.mockito.ArgumentMatchers.any(Throwable.class));
    assertThat(engine.currentGeneration()).isNull(); // close cleared it, as always
  }

  @Test
  void a304KeepsTheCurrentGenerationQuietly() {
    backend = new TestBackend();
    byte[] module = TestBackend.module("basic.wasm");
    backend.publish(module, "tok-1", "\"v1\"");
    engine = newEngine(backend, null);
    engine.start(Duration.ofSeconds(1));
    Engine.Generation generation = engine.currentGeneration();

    // Republishing the same ETag answers 304: the steady state after a module installs. It must
    // neither fail (the old code logged a stack trace every poll) nor swap the generation.
    backend.publish(module, "tok-1", "\"v1\"");
    Awaitility.await()
        .atMost(5, TimeUnit.SECONDS)
        .until(() -> backend.ifNoneMatch().size() >= 2); // a conditional fetch happened
    Awaitility.await()
        .during(2, TimeUnit.SECONDS) // and stayed quiet for further polls
        .atMost(5, TimeUnit.SECONDS)
        .until(() -> engine.currentGeneration() == generation);
    evaluate(engine, "u");
  }

  @Test
  void startup404ThenPublishedBecomesReady() throws Exception {
    backend = new TestBackend();
    backend.setStatus(404);
    engine = newEngine(backend, null);
    CompletableFuture.runAsync(
        () -> {
          try {
            Thread.sleep(30);
          } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
          }
          backend.publish(TestBackend.module("basic.wasm"), "tok", "\"v\"");
        });
    engine.start(Duration.ofSeconds(5));
    evaluate(engine, "u");
  }

  @Test
  void aForbiddenCredentialIsTerminalAtStartup() {
    backend = new TestBackend();
    backend.setStatus(403);
    engine = newEngine(backend, null);
    assertThatThrownBy(() -> engine.start(Duration.ofSeconds(1)))
        .isInstanceOf(TerminalStartupException.class);
  }

  @Test
  void anInvalidFirstModuleIsTerminalAtStartup() {
    backend = new TestBackend();
    backend.publish("not wasm".getBytes(), "tok", "\"v\"");
    engine = newEngine(backend, null);
    assertThatThrownBy(() -> engine.start(Duration.ofSeconds(1)))
        .isInstanceOf(TerminalStartupException.class);
  }

  @Test
  void aStartupTimeoutIsRecoverableAndOnReadyFiresLater() {
    backend = new TestBackend();
    backend.setStatus(503);
    CountDownLatch ready = new CountDownLatch(1);
    engine = newEngine(backend, c -> c.onReady(ready::countDown));
    assertThatThrownBy(() -> engine.start(Duration.ofMillis(30)))
        .isInstanceOf(StartupTimeoutException.class)
        .isNotInstanceOf(TerminalStartupException.class);
    backend.publish(TestBackend.module("basic.wasm"), "tok", "\"v\"");
    try {
      assertThat(ready.await(5, TimeUnit.SECONDS)).isTrue();
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new IllegalStateException(e);
    }
    evaluate(engine, "u");
  }

  /** The store that blocks reads until released, as the Go blockingStore does. */
  static final class BlockingStore implements MaterializationStore {

    final CountDownLatch entered = new CountDownLatch(1);
    final CountDownLatch release = new CountDownLatch(1);

    @Override
    public List<Record> read(List<Record> requested) {
      entered.countDown();
      try {
        release.await();
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
      return List.of();
    }
  }

  @Test
  void closeHonorsTheDeadlineWithAReadInFlightAndCleansUpAfterwards() throws Exception {
    backend = new TestBackend();
    backend.publish(TestBackend.module("basic.wasm"), "tok-1", "\"v1\"");
    engine = newEngine(backend, null);
    engine.start(Duration.ofSeconds(1));

    BlockingStore store = new BlockingStore();
    var evaluation =
        CompletableFuture.runAsync(
            () ->
                engine.evaluate(
                    request("u", "flags/sticky"),
                    List.of(
                        EvaluateRequest.newBuilder()
                            .setFlagKey("sticky.value")
                            .setDefaultValue(Value.newBuilder().setNumberValue(-1).build())
                            .build()),
                    store));
    assertThat(store.entered.await(5, TimeUnit.SECONDS)).isTrue();

    long started = System.nanoTime();
    assertThat(engine.close(Duration.ofMillis(20))).isFalse();
    assertThat(System.nanoTime() - started).isLessThan(Duration.ofSeconds(1).toNanos());
    assertThatThrownBy(() -> evaluate(engine, "u")).isInstanceOf(EngineClosedException.class);

    store.release.countDown();
    evaluation.get(5, TimeUnit.SECONDS); // the in-flight evaluation finished
    Awaitility.await()
        .atMost(5, TimeUnit.SECONDS)
        .until(() -> tokens(backend).getOrDefault("tok-1", 0) > 0);
  }
}
