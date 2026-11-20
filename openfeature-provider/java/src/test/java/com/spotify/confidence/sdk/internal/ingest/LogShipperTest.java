package com.spotify.confidence.sdk.internal.ingest;

import static org.assertj.core.api.Assertions.assertThat;

import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.http.HttpClient;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** Ports the Go shipper tests: verbatim delivery, retries, drops, and the close deadline. */
class LogShipperTest {

  private final List<FakeIngest> servers = new ArrayList<>();

  @AfterEach
  void stop() {
    servers.forEach(FakeIngest::close);
  }

  private LogShipper newShipper(int queueSize) {
    FakeIngest server = new FakeIngest();
    servers.add(server);
    LogShipper shipper = new LogShipper(server.url(), server.client(), queueSize, t -> {});
    shipper.backoffMillis = 1;
    return shipper;
  }

  @Test
  void shipsVerbatimWithToken() {
    FakeIngest server = new FakeIngest();
    servers.add(server);
    LogShipper shipper = new LogShipper(server.url(), server.client(), 0, t -> {});
    shipper.backoffMillis = 1;
    assertThat(shipper.enqueue("batch-1".getBytes(), "tok-a")).isTrue();
    assertThat(shipper.enqueue("batch-2".getBytes(), "tok-b")).isTrue();
    assertThat(shipper.close(Duration.ofSeconds(10))).isTrue();
    assertThat(server.requests())
        .containsExactly(
            new Request("/v1/flagLogs:write", "Bearer tok-a", "application/x-protobuf", "batch-1"),
            new Request("/v1/flagLogs:write", "Bearer tok-b", "application/x-protobuf", "batch-2"));
  }

  @Test
  void retriesRetryableStatusesAndDropsRejections() {
    record Case(String name, List<Integer> statuses, int requests, long failed) {}
    List<Case> cases =
        List.of(
            new Case("5xx then ok", List.of(503, 500), 3, 0),
            new Case("429 then ok", List.of(429), 2, 0),
            new Case("exhausted", List.of(503, 503, 503), 3, 1),
            new Case("4xx is not retried", List.of(400), 1, 1));
    for (Case c : cases) {
      FakeIngest server = new FakeIngest(c.statuses(), false);
      servers.add(server);
      LogShipper shipper = new LogShipper(server.url(), server.client(), 0, t -> {});
      shipper.backoffMillis = 1;
      assertThat(shipper.enqueue("b".getBytes(), "t")).isTrue();
      assertThat(shipper.close(Duration.ofSeconds(10))).as(c.name()).isTrue();
      assertThat(server.requests()).as(c.name()).hasSize(c.requests());
      assertThat(shipper.failed()).as(c.name()).isEqualTo(c.failed());
    }
  }

  @Test
  void aFullQueueDropsWithoutBlocking() throws Exception {
    FakeIngest server = new FakeIngest(List.of(), true); // every request blocks until released
    servers.add(server);
    LogShipper shipper = new LogShipper(server.url(), server.client(), 1, t -> {});
    shipper.backoffMillis = 1;
    int sent = 0;
    for (int i = 0; i < 10; i++) {
      if (shipper.enqueue("b".getBytes(), "t")) {
        sent++;
      }
    }
    // One batch in flight (blocked in the server), one queued; the rest dropped.
    assertThat(shipper.dropped()).isGreaterThanOrEqualTo(8);
    assertThat(sent + shipper.dropped()).isEqualTo(10);
    server.release();
    assertThat(shipper.close(Duration.ofSeconds(10))).isTrue();
    assertThat(shipper.enqueue("late".getBytes(), "t"))
        .as("enqueue after close accepted")
        .isFalse();
    assertThat(shipper.dropped()).isEqualTo(10 - sent + 1);
  }

  @Test
  void closeHonoursTheDeadline() throws Exception {
    FakeIngest server = new FakeIngest(List.of(), true); // stalls the delivery thread
    servers.add(server);
    LogShipper shipper = new LogShipper(server.url(), server.client(), 0, t -> {});
    shipper.backoffMillis = 1;
    assertThat(shipper.enqueue("b".getBytes(), "t")).isTrue();
    long started = System.nanoTime();
    boolean drained = shipper.close(Duration.ofMillis(20));
    long waited = System.nanoTime() - started;
    assertThat(drained).isFalse();
    assertThat(waited).isLessThan(Duration.ofSeconds(5).toNanos()); // didn't wait for the post
    server.release();
    // The delivery thread finishes on its own; the shipper is done draining afterwards.
    assertThat(shipper.close(Duration.ofSeconds(10))).isTrue();
  }

  // ---------------------------------------------------------------- the fake ingestion server

  record Request(String path, String auth, String contentType, String body) {}

  /** Serves statuses in order (200 once exhausted), records requests, and can stall. */
  private static final class FakeIngest implements AutoCloseable {

    private final HttpServer server;
    private final HttpClient client = HttpClient.newHttpClient();
    private final List<Integer> statuses = new ArrayList<>();
    private final List<Request> requests = new ArrayList<>();
    private final CountDownLatch release = new CountDownLatch(1);
    private final boolean stall;

    FakeIngest(Integer... statuses) {
      this(List.of(statuses), false);
    }

    FakeIngest(List<Integer> initialStatuses, boolean stall) {
      this.statuses.addAll(initialStatuses);
      this.stall = stall;
      try {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
      } catch (IOException e) {
        throw new IllegalStateException("bind", e);
      }
      server.createContext(
          "/",
          exchange -> {
            if (stall) {
              try {
                release.await();
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
            }
            byte[] body = exchange.getRequestBody().readAllBytes();
            int status = statuses.isEmpty() ? 200 : statuses.remove(0);
            synchronized (requests) {
              requests.add(
                  new Request(
                      exchange.getRequestURI().getPath(),
                      exchange.getRequestHeaders().getFirst("Authorization"),
                      exchange.getRequestHeaders().getFirst("Content-Type"),
                      new String(body, StandardCharsets.UTF_8)));
            }
            exchange.sendResponseHeaders(status, -1);
            exchange.close();
          });
      server.start();
    }

    List<Request> requests() {
      synchronized (requests) {
        return List.copyOf(requests);
      }
    }

    void release() {
      release.countDown();
    }

    String url() {
      return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    HttpClient client() {
      return client;
    }

    @Override
    public void close() {
      server.stop(0);
    }
  }
}
