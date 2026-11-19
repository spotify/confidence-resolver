package com.spotify.confidence.sdk.internal.testutil;

import com.google.protobuf.ByteString;
import com.spotify.confidence.flags.wasm.v1.ClientModule;
import com.spotify.confidence.flags.wasm.v1.WriteFlagLogsRequest;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.InputStream;
import java.net.InetSocketAddress;
import java.net.http.HttpClient;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * Fakes the Confidence backend — the module CDN and flag-log ingestion — for tests. Port of the Go
 * provider's internal/testutil Backend; that file is the reference.
 */
public final class TestBackend implements AutoCloseable {

  /** The client secret of the shared test states. */
  public static final String SECRET = "test-secret";

  /** Reads a compiled module from openfeature-provider/testdata/modules. */
  public static byte[] module(String name) {
    try (InputStream in = TestBackend.class.getResourceAsStream("/wasm/" + name)) {
      if (in != null) {
        return in.readAllBytes();
      }
      return Files.readAllBytes(Path.of("..", "testdata", "modules", name));
    } catch (IOException e) {
      throw new IllegalStateException("read fixture module " + name, e);
    }
  }

  /** One received flag-log request. */
  public record Batch(String token, byte[] body) {}

  private final byte[] key = new byte[32];
  private final HttpServer cdn;
  private final HttpServer ingest;
  private final HttpClient client = HttpClient.newHttpClient();

  private volatile int status = 404;
  private volatile byte[] module = new byte[0];
  private volatile String etag = "";
  private final List<String> ifNoneMatch = new CopyOnWriteArrayList<>();
  private final List<Batch> batches = new CopyOnWriteArrayList<>();
  private final SecureRandom random = new SecureRandom();

  public TestBackend() {
    random.nextBytes(key);
    cdn =
        server(
            "/modules/",
            exchange -> {
              String inm = exchange.getRequestHeaders().getFirst("If-None-Match");
              ifNoneMatch.add(inm == null ? "" : inm);
              int code = status;
              byte[] body = new byte[0];
              if (code == 200 && !inm.isEmpty() && inm.equals(etag)) {
                code = 304;
              } else if (code == 200) {
                exchange.getResponseHeaders().add("ETag", etag);
                body = module;
              }
              respond(exchange, code, body);
            });
    ingest =
        server(
            "/v1/",
            exchange -> {
              byte[] body = exchange.getRequestBody().readAllBytes();
              String auth = exchange.getRequestHeaders().getFirst("Authorization");
              batches.add(
                  new Batch(
                      auth != null && auth.startsWith("Bearer ")
                          ? auth.substring("Bearer ".length())
                          : "",
                      body));
              respond(exchange, 200, new byte[0]);
            });
  }

  /** Serves wasm, with token, under etag. */
  public void publish(byte[] wasm, String token, String etag) {
    try {
      byte[] plain =
          ClientModule.newBuilder()
              .setModule(ByteString.copyFrom(wasm))
              .setLogToken(token)
              .build()
              .toByteArray();
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      byte[] nonce = new byte[12];
      random.nextBytes(nonce);
      cipher.init(
          Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, nonce));
      byte[] sealed = cipher.doFinal(plain);
      byte[] artifact = new byte[nonce.length + sealed.length];
      System.arraycopy(nonce, 0, artifact, 0, nonce.length);
      System.arraycopy(sealed, 0, artifact, nonce.length, sealed.length);
      this.status = 200;
      this.module = artifact;
      this.etag = etag;
    } catch (Exception e) {
      throw new IllegalStateException("publish", e);
    }
  }

  /** Makes the CDN answer with status instead of a module. */
  public void setStatus(int status) {
    this.status = status;
  }

  /** The If-None-Match header of every CDN request so far. */
  public List<String> ifNoneMatch() {
    return List.copyOf(ifNoneMatch);
  }

  /** The flag-log batches received so far. */
  public List<Batch> batches() {
    return List.copyOf(batches);
  }

  /** Decodes every received batch. */
  public List<WriteFlagLogsRequest> logs() {
    List<WriteFlagLogsRequest> out = new ArrayList<>();
    for (Batch batch : batches) {
      try {
        out.add(WriteFlagLogsRequest.parseFrom(batch.body()));
      } catch (IOException e) {
        throw new IllegalStateException("malformed batch", e);
      }
    }
    return out;
  }

  /** The fake CDN's base URL. */
  public String cdnUrl() {
    return "http://127.0.0.1:" + cdn.getAddress().getPort();
  }

  /** The fake ingestion's base URL. */
  public String ingestUrl() {
    return "http://127.0.0.1:" + ingest.getAddress().getPort();
  }

  /** An HttpClient talking to the fakes. */
  public HttpClient client() {
    return client;
  }

  /** The key as the provider's 64-hex-character string. */
  public String hexKey() {
    StringBuilder out = new StringBuilder(64);
    for (byte b : key) {
      out.append(String.format("%02x", b));
    }
    return out.toString();
  }

  @Override
  public void close() {
    cdn.stop(0);
    ingest.stop(0);
  }

  private static HttpServer server(String context, Handler handler) {
    try {
      HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
      server.createContext(
          context,
          exchange -> {
            handler.handle(exchange);
            exchange.close();
          });
      server.start();
      return server;
    } catch (IOException e) {
      throw new IllegalStateException("bind", e);
    }
  }

  private interface Handler {

    void handle(com.sun.net.httpserver.HttpExchange exchange) throws IOException;
  }

  private static void respond(com.sun.net.httpserver.HttpExchange exchange, int status, byte[] body)
      throws IOException {
    if (status == 304 || body.length == 0) {
      exchange.sendResponseHeaders(status, -1);
      return;
    }
    exchange.sendResponseHeaders(status, body.length);
    try (java.io.OutputStream out = exchange.getResponseBody()) {
      out.write(body);
    }
  }
}
