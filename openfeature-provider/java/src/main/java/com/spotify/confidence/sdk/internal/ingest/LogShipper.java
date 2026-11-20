package com.spotify.confidence.sdk.internal.ingest;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;

/**
 * Sends batches from a bounded queue on a background thread. Batches the queue can't hold are
 * dropped and counted rather than blocking the caller, which may be inside a WASM call.
 *
 * <p>Port of the Go provider's internal/ingest shipper; that file is the reference.
 */
public final class LogShipper {

  /** The regionless production ingestion host. */
  public static final String DEFAULT_BASE_URL = "https://ingestion.confidence.dev";

  private static final String PATH = "/v1/flagLogs:write";
  private static final Duration ATTEMPT_TIMEOUT = Duration.ofSeconds(5);
  private static final int MAX_ATTEMPTS = 3;

  /** For tests: the backoff between attempts, in milliseconds. */
  volatile long backoffMillis = 200;

  private record Batch(byte[] body, String token) {}

  private final String url;
  private final HttpClient client;
  private final ArrayBlockingQueue<Batch> queue;
  private final CountDownLatch done = new CountDownLatch(1);
  private final AtomicBoolean closed = new AtomicBoolean();
  private final Object enqueueLock = new Object();
  private final AtomicLong dropped = new AtomicLong();
  private final AtomicLong failed = new AtomicLong();
  private final Consumer<Throwable> report;

  /**
   * Creates a shipper; baseUrl may be empty (production ingestion), client null (a default one),
   * queueSize zero (64 batches), report null (ignore delivery failures).
   */
  public LogShipper(String baseUrl, HttpClient client, int queueSize, Consumer<Throwable> report) {
    this.url = (baseUrl == null || baseUrl.isEmpty() ? DEFAULT_BASE_URL : baseUrl) + PATH;
    this.client = client == null ? HttpClient.newHttpClient() : client;
    this.queue = new ArrayBlockingQueue<>(queueSize <= 0 ? 64 : queueSize);
    this.report = report == null ? t -> {} : report;
    Thread delivery = new Thread(this::run, "confidence-log-shipper");
    delivery.setDaemon(true);
    delivery.start();
  }

  /**
   * Queues body for delivery with token, without blocking. Returns false if the batch was dropped
   * because the queue is full or the shipper is closed.
   */
  public boolean enqueue(byte[] body, String token) {
    synchronized (enqueueLock) {
      if (closed.get()) {
        dropped.incrementAndGet();
        return false;
      }
      if (queue.offer(new Batch(body, token))) {
        return true;
      }
    }
    dropped.incrementAndGet();
    return false;
  }

  /** Batches never sent because the queue was full or closed. */
  public long dropped() {
    return dropped.get();
  }

  /** Batches given up on after a rejection or exhausted retries. */
  public long failed() {
    return failed.get();
  }

  /**
   * Stops accepting batches and waits for the queue to drain, up to the timeout. Returns false when
   * the timeout ended before the queue drained.
   */
  public boolean close(Duration timeout) {
    synchronized (enqueueLock) {
      closed.set(true);
    }
    try {
      return done.await(timeout.toNanos(), TimeUnit.NANOSECONDS);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      return false;
    }
  }

  private void run() {
    try {
      while (true) {
        Batch batch = queue.poll(100, TimeUnit.MILLISECONDS);
        if (batch != null) {
          Throwable error = send(batch);
          if (error != null) {
            failed.incrementAndGet();
            report.accept(error);
          }
        } else if (closed.get() && queue.isEmpty()) {
          return;
        }
      }
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
    } finally {
      done.countDown();
    }
  }

  /** Retries retryable failures (429, 5xx, network) up to {@link #MAX_ATTEMPTS} times. */
  private Throwable send(Batch batch) {
    Throwable error = null;
    for (int attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
      if (attempt > 0) {
        try {
          Thread.sleep(backoffMillis << (attempt - 1));
        } catch (InterruptedException e) {
          Thread.currentThread().interrupt();
          return e;
        }
      }
      error = post(batch);
      if (error == null || !retryable(error)) {
        return error;
      }
    }
    return error;
  }

  static boolean retryable(Throwable error) {
    return error instanceof IOException; // network problems; status codes carry the flag
  }

  /** Posts one batch; a retryable flag is carried by the exception's subtype. */
  private Throwable post(Batch batch) {
    HttpRequest request =
        HttpRequest.newBuilder(URI.create(url))
            .timeout(ATTEMPT_TIMEOUT)
            .header("Content-Type", "application/x-protobuf")
            .header("Authorization", "Bearer " + batch.token())
            .POST(HttpRequest.BodyPublishers.ofByteArray(batch.body()))
            .build();
    try {
      HttpResponse<Void> response = client.send(request, HttpResponse.BodyHandlers.discarding());
      int status = response.statusCode();
      if (status < 300) {
        return null;
      }
      String message = "flag logs: HTTP " + status;
      if (status == 429 || status >= 500) {
        return new IOException(message); // retryable
      }
      return new RuntimeException("flag logs rejected: HTTP " + status);
    } catch (IOException e) {
      return new IOException("flag logs: " + e.getMessage(), e); // retryable
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      return e;
    }
  }
}
