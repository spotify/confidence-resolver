package com.spotify.confidence.sdk.internal.host;

import static org.assertj.core.api.Assertions.assertThat;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import com.spotify.confidence.flags.wasm.v1.WriteFlagLogsRequest;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

/** Shared helpers for the host tests, mirroring the Go host's test file. */
final class HostTestSupport {

  static final String TEST_SECRET = "test-secret";
  static final Path BASIC_WASM = Path.of("..", "testdata", "modules", "basic.wasm");

  private HostTestSupport() {}

  static byte[] fixture() {
    try {
      return Files.readAllBytes(BASIC_WASM);
    } catch (IOException e) {
      throw new IllegalStateException("read fixture: " + BASIC_WASM, e);
    }
  }

  static Resolver newResolver(boolean interpreter, HostConfig.Builder config) {
    Module module = Module.parse(fixture());
    return new Resolver(module, config.interpreter(interpreter).build());
  }

  /** Runs the body on both engines, as the Go tests' bothEngines does. */
  static void bothEngines(java.util.function.Consumer<Boolean> body) {
    body.accept(false); // runtime compiler
    body.accept(true); // interpreter
  }

  static ResolveFlagsRequest request(String unit, String... flags) {
    return ResolveFlagsRequest.newBuilder()
        .addAllFlags(List.of(flags))
        .setEvaluationContext(
            Struct.newBuilder()
                .putFields("targeting_key", Value.newBuilder().setStringValue(unit).build()))
        .setClientSecret(TEST_SECRET)
        .setApply(true)
        .build();
  }

  static List<ResolutionDetails> evaluate(
      Resolver resolver,
      ResolveFlagsRequest req,
      MaterializationStore store,
      EvaluateRequest... evals) {
    return resolver.evaluate(req, List.of(evals), store);
  }

  /** The store the Go tests use: fixed records, a read counter, and collected writes. */
  static final class FakeStore implements MaterializationStore {

    final List<Record> records;
    final AtomicInteger reads = new AtomicInteger();
    final List<Record> written = new CopyOnWriteArrayList<>();

    FakeStore(Record... records) {
      // a single null element (e.g. from a conditional argument) means "no records"
      this.records =
          records == null || records.length == 0
              ? List.of()
              : java.util.Arrays.stream(records).filter(java.util.Objects::nonNull).toList();
    }

    @Override
    public List<Record> read(List<Record> requested) {
      reads.incrementAndGet();
      return records;
    }

    @Override
    public void write(List<Record> assignments) {
      written.addAll(assignments);
    }
  }

  static final Record STICKY_RECORD =
      Record.newBuilder()
          .setUnit("u1")
          .setMaterialization("materializedSegments/sticky")
          .setRule("flags/sticky/rules/paused")
          .setVariant("flags/sticky/variants/sticky")
          .build();

  /** Counts log batches and assigned flags, as the Go tests' logSink does. */
  static final class LogSink {

    final List<byte[]> batches = new CopyOnWriteArrayList<>();

    void write(byte[] batch) {
      batches.add(batch);
    }

    int assigned() {
      int assigned = 0;
      for (byte[] batch : batches) {
        try {
          WriteFlagLogsRequest request = WriteFlagLogsRequest.parseFrom(batch);
          for (var fa : request.getFlagAssignedList()) {
            assigned += fa.getFlagsCount();
          }
        } catch (IOException e) {
          throw new IllegalStateException("malformed log batch", e);
        }
      }
      return assigned;
    }

    void assertThatLogsAreWellFormed() {
      for (byte[] batch : batches) {
        try {
          assertThat(WriteFlagLogsRequest.parseFrom(batch)).isNotNull();
        } catch (IOException e) {
          throw new IllegalStateException("malformed log batch", e);
        }
      }
    }
  }
}
