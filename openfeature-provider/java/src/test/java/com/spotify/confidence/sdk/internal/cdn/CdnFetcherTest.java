package com.spotify.confidence.sdk.internal.cdn;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.ByteString;
import com.spotify.confidence.flags.wasm.v1.ClientModule;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.http.HttpClient;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.util.HexFormat;
import java.util.List;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** Ports the Go fetcher tests: ETag flow, decryption, and error classification. */
class CdnFetcherTest {

  private static final String SECRET = "test-secret";

  private final SecureRandom random = new SecureRandom();
  private FakeCdn cdn;

  @AfterEach
  void stop() {
    if (cdn != null) {
      cdn.close();
    }
  }

  @Test
  void fetchesDecryptsAndUsesETags() {
    byte[] key = newKey();
    byte[] artifact = encrypt(key, module(new byte[] {0, 'a', 's', 'm'}, "jwt"));
    cdn = new FakeCdn(200, artifact, "\"v1\"");
    CdnFetcher fetcher = cdn.fetcher(key);

    CdnFetcher.FetchedModule module = fetcher.fetch();
    assertThat(cdn.lastPath).isEqualTo("/modules/" + sha256Hex(SECRET) + ".wasm.enc");
    assertThat(module.wasm()).startsWith(new byte[] {0, 'a', 's', 'm'});
    assertThat(module.logToken()).isEqualTo("jwt");
    assertThat(module.etag()).isEqualTo("\"v1\"");

    // Not accepted yet (e.g. failed to install): fetch again unconditionally.
    fetcher.fetch();
    assertThat(cdn.lastIfNoneMatch).isEmpty();

    fetcher.accept(module.etag());
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotModifiedException.class);
    assertThat(cdn.lastIfNoneMatch).isEqualTo("\"v1\"");
  }

  @Test
  void classifiesFailures() {
    byte[] key = newKey();
    byte[] good = encrypt(key, module(new byte[] {'w', 'a', 's', 'm'}, "jwt"));
    byte[] tampered = good.clone();
    tampered[tampered.length - 1] ^= 1;

    record Case(String name, int status, byte[] body, byte[] fetchKey, Class<?> type) {}
    List<Case> cases =
        List.of(
            new Case("not published", 404, new byte[0], key, NotPublishedException.class),
            new Case("server error", 503, new byte[0], key, CdnException.class),
            new Case("rate limited", 429, new byte[0], key, CdnException.class),
            new Case("request timeout", 408, new byte[0], key, CdnException.class),
            new Case("forbidden", 403, new byte[0], key, TerminalCdnException.class),
            new Case("wrong key", 200, good, newKey(), TerminalCdnException.class),
            new Case("tampered", 200, tampered, key, TerminalCdnException.class),
            new Case("truncated", 200, trim(good, 10), key, TerminalCdnException.class),
            new Case(
                "no log token",
                200,
                encrypt(key, module(new byte[] {'w', 'a', 's', 'm'}, "")),
                key,
                TerminalCdnException.class));
    for (Case c : cases) {
      cdn = new FakeCdn(c.status(), c.body(), null);
      CdnFetcher fetcher = cdn.fetcher(c.fetchKey());
      assertThatThrownBy(fetcher::fetch)
          .as(c.name())
          .isInstanceOf(c.type())
          .hasMessageNotContaining(HexFormat.of().formatHex(c.fetchKey()));
    }
  }

  @Test
  void parsesValidatingKeys() {
    String valid = "aB".repeat(32);
    assertThat(CdnFetcher.parseKey(valid)).hasSize(32);
    for (String bad :
        java.util.Arrays.asList(
            "", "a".repeat(63), "g".repeat(64), " " + valid.substring(1), (String) null)) {
      assertThatThrownBy(() -> CdnFetcher.parseKey(bad))
          .as("key %s", bad == null ? "null" : "len " + bad.length())
          .isInstanceOf(IllegalArgumentException.class);
    }
  }

  // ---------------------------------------------------------------- helpers

  private static ClientModule module(byte[] wasm, String logToken) {
    return ClientModule.newBuilder()
        .setModule(ByteString.copyFrom(wasm))
        .setLogToken(logToken)
        .build();
  }

  private byte[] newKey() {
    byte[] key = new byte[32];
    random.nextBytes(key);
    return key;
  }

  private byte[] encrypt(byte[] key, ClientModule module) {
    try {
      byte[] plain = module.toByteArray();
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      byte[] nonce = new byte[12];
      random.nextBytes(nonce);
      cipher.init(
          Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, nonce));
      byte[] sealed = cipher.doFinal(plain);
      byte[] out = new byte[nonce.length + sealed.length];
      System.arraycopy(nonce, 0, out, 0, nonce.length);
      System.arraycopy(sealed, 0, out, nonce.length, sealed.length);
      return out;
    } catch (Exception e) {
      throw new IllegalStateException("encrypt", e);
    }
  }

  private static byte[] trim(byte[] bytes, int length) {
    byte[] out = new byte[length];
    System.arraycopy(bytes, 0, out, 0, length);
    return out;
  }

  private static String sha256Hex(String value) {
    try {
      byte[] sum =
          MessageDigest.getInstance("SHA-256").digest(value.getBytes(StandardCharsets.UTF_8));
      return HexFormat.of().formatHex(sum);
    } catch (Exception e) {
      throw new IllegalStateException(e);
    }
  }

  /** The fake CDN the Go tests serve with httptest. */
  private final class FakeCdn implements AutoCloseable {

    final HttpServer server;
    final HttpClient client = HttpClient.newHttpClient();
    volatile int status;
    volatile byte[] body;
    volatile String etag;
    volatile String lastPath;
    volatile String lastIfNoneMatch;

    FakeCdn(int status, byte[] body, String etag) {
      this.status = status;
      this.body = body;
      this.etag = etag;
      try {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
      } catch (IOException e) {
        throw new IllegalStateException("bind", e);
      }
      server.createContext(
          "/",
          exchange -> {
            lastPath = exchange.getRequestURI().getPath();
            lastIfNoneMatch =
                exchange.getRequestHeaders().getFirst("If-None-Match") == null
                    ? ""
                    : exchange.getRequestHeaders().getFirst("If-None-Match");
            byte[] response = new byte[0];
            int code = status;
            if (etag != null && !etag.isEmpty() && etag.equals(lastIfNoneMatch)) {
              code = 304;
            } else if (etag != null && !etag.isEmpty()) {
              exchange.getResponseHeaders().add("ETag", etag);
              response = body;
            } else {
              response = body;
            }
            exchange.sendResponseHeaders(code, code == 304 ? -1 : response.length);
            if (code != 304 && response.length > 0) {
              try (OutputStream out = exchange.getResponseBody()) {
                out.write(response);
              }
            }
            exchange.close();
          });
      server.start();
    }

    CdnFetcher fetcher(byte[] key) {
      return new CdnFetcher(SECRET, key, "http://127.0.0.1:" + port(), client);
    }

    int port() {
      return server.getAddress().getPort();
    }

    @Override
    public void close() {
      server.stop(0);
    }
  }
}
