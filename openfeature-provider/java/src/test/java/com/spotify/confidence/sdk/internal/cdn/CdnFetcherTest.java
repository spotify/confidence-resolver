package com.spotify.confidence.sdk.internal.cdn;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.ByteString;
import com.spotify.confidence.flags.wasm.v1.ClientModule;
import com.spotify.confidence.sdk.internal.testutil.TestBackend;
import com.sun.net.httpserver.HttpHandler;
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
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** Ports the Go fetcher tests: ETag flow, decryption, and error classification. */
class CdnFetcherTest {

  private static final String SECRET = "test-secret";
  private static final String PROFILE = "TEST_PROFILE_1";

  private final SecureRandom random = new SecureRandom();
  private FakeCdn cdn;
  private HttpServer resolver;

  @AfterEach
  void stop() {
    if (cdn != null) {
      cdn.close();
    }
    if (resolver != null) {
      resolver.stop(0);
    }
  }

  @Test
  void fetchesDecryptsAndUsesETags() {
    byte[] key = newKey();
    byte[] artifact = encrypt(key, module(new byte[] {0, 'a', 's', 'm'}, "jwt"));
    cdn = new FakeCdn(200, artifact, "\"v1\"");
    CdnFetcher fetcher = cdn.fetcher(key);

    CdnFetcher.FetchedModule module = fetcher.fetch();
    assertThat(cdn.lastPath)
        .isEqualTo("/modules/" + sha256Hex(SECRET) + "/test_profile_1.wasm.enc");
    assertThat(cdn.lastAuthorization).isNull();
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
    AtomicInteger registrations = new AtomicInteger();
    startResolver(
        exchange -> {
          registrations.incrementAndGet();
          exchange.sendResponseHeaders(500, -1);
          exchange.close();
        });
    for (Case c : cases) {
      try (FakeCdn fake = new FakeCdn(c.status(), c.body(), null)) {
        CdnFetcher fetcher = fake.fetcher(c.fetchKey());
        assertThatThrownBy(fetcher::fetch)
            .as(c.name())
            .isExactlyInstanceOf(c.type())
            .hasMessageNotContaining(HexFormat.of().formatHex(c.fetchKey()));
      }
    }
    assertThat(registrations).hasValue(0);
  }

  @Test
  void registersOnceAndChecksTheCdnOnEveryPoll() {
    byte[] key = newKey();
    AtomicInteger registrations = new AtomicInteger();
    AtomicReference<List<String>> request = new AtomicReference<>();
    startResolver(
        exchange -> {
          registrations.incrementAndGet();
          request.set(
              List.of(
                  exchange.getRequestMethod(),
                  exchange.getRequestURI().getPath(),
                  exchange.getRequestHeaders().getFirst("Authorization"),
                  exchange.getRequestHeaders().getFirst("Content-Type"),
                  new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8)));
          byte[] ack = "{\"cdnRetryDelay\":\"1.25s\"}".getBytes(StandardCharsets.UTF_8);
          exchange.sendResponseHeaders(200, ack.length);
          exchange.getResponseBody().write(ack);
          exchange.close();
        });
    cdn = new FakeCdn(404, new byte[0], null);
    CdnFetcher fetcher =
        new CdnFetcher(SECRET, key, cdn.url(), PROFILE, TestBackend.withMockRegistration(resolver));

    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class);
    assertThat(request.get())
        .containsExactly(
            "POST",
            "/v1/profile:register",
            "ClientSecret " + SECRET,
            "application/json",
            "{\"profile\":\"TEST_PROFILE_1\"}");
    assertThat(cdn.requests).hasValue(1);

    // The next scheduled attempt reaches the CDN even before the response's delay expires.
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class);
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class);
    assertThat(cdn.requests).hasValue(3);
    cdn.body = encrypt(key, module(new byte[] {1, 2, 3}, "jwt"));
    cdn.status = 200;
    assertThat(fetcher.fetch().wasm()).containsExactly(1, 2, 3);

    cdn.status = 404;
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class);
    assertThat(cdn.requests).hasValue(5);
    assertThat(registrations).hasValue(1);
  }

  @Test
  void registrationRejectsRedirects() {
    AtomicInteger redirectHits = new AtomicInteger();
    startResolver(
        exchange -> {
          if (exchange.getRequestURI().getPath().equals("/v1/profile:register")) {
            exchange.getResponseHeaders().set("Location", "/redirected");
            exchange.sendResponseHeaders(307, -1);
          } else {
            redirectHits.incrementAndGet();
            exchange.sendResponseHeaders(500, -1);
          }
          exchange.close();
        });
    cdn = new FakeCdn(404, new byte[0], null);
    CdnFetcher fetcher = cdn.fetcher(newKey());

    assertThatThrownBy(fetcher::fetch)
        .isInstanceOf(NotPublishedException.class)
        .hasCauseInstanceOf(CdnException.class)
        .hasRootCauseMessage("module profile registration: 307");
    assertThat(redirectHits).hasValue(0);
  }

  @Test
  void registrationUsesANonRedirectingClientEvenWhenTheProviderFollowsRedirects() {
    HttpClient redirecting =
        HttpClient.newBuilder().followRedirects(HttpClient.Redirect.ALWAYS).build();
    assertThat(ProfileRegistration.withoutRedirects(redirecting).followRedirects())
        .isEqualTo(HttpClient.Redirect.NEVER);
  }

  @Test
  void failedRegistrationIsRetriedOnTheNextCdnMissUntilSuccess() {
    byte[] key = newKey();
    AtomicInteger registrations = new AtomicInteger();
    startResolver(
        exchange -> {
          int attempt = registrations.incrementAndGet();
          exchange.sendResponseHeaders(attempt == 1 ? 403 : 204, -1);
          exchange.close();
        });
    cdn = new FakeCdn(404, new byte[0], null);
    CdnFetcher fetcher =
        new CdnFetcher(SECRET, key, cdn.url(), PROFILE, TestBackend.withMockRegistration(resolver));

    assertThatThrownBy(fetcher::fetch)
        .isExactlyInstanceOf(NotPublishedException.class)
        .hasRootCauseMessage("module profile registration: 403");
    assertThat(registrations).hasValue(1);
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class).hasNoCause();
    assertThat(registrations).hasValue(2);
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class).hasNoCause();
    assertThat(cdn.requests).hasValue(3);
    cdn.body = encrypt(key, module(new byte[] {1, 2, 3}, "jwt"));
    cdn.status = 200;
    assertThat(fetcher.fetch().wasm()).containsExactly(1, 2, 3);
    assertThat(registrations).hasValue(2);
  }

  @Test
  void acceptsRegistrationWithoutAResponseBody() {
    startResolver(
        exchange -> {
          exchange.sendResponseHeaders(200, -1);
          exchange.close();
        });
    cdn = new FakeCdn(404, new byte[0], null);
    CdnFetcher fetcher = cdn.fetcher(newKey());
    assertThatThrownBy(fetcher::fetch).isInstanceOf(NotPublishedException.class);
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

  private void startResolver(HttpHandler handler) {
    try {
      resolver = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    } catch (IOException e) {
      throw new IllegalStateException("bind", e);
    }
    resolver.createContext("/", handler);
    resolver.start();
  }

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
    volatile int status;
    volatile byte[] body;
    volatile String etag;
    volatile String lastPath;
    volatile String lastIfNoneMatch;
    volatile String lastAuthorization;
    final AtomicInteger requests = new AtomicInteger();

    FakeCdn(int initialStatus, byte[] initialBody, String etag) {
      this.status = initialStatus;
      this.body = initialBody;
      this.etag = etag;
      try {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
      } catch (IOException e) {
        throw new IllegalStateException("bind", e);
      }
      server.createContext(
          "/",
          exchange -> {
            requests.incrementAndGet();
            lastPath = exchange.getRequestURI().getPath();
            lastAuthorization = exchange.getRequestHeaders().getFirst("Authorization");
            lastIfNoneMatch =
                exchange.getRequestHeaders().getFirst("If-None-Match") == null
                    ? ""
                    : exchange.getRequestHeaders().getFirst("If-None-Match");
            byte[] response = new byte[0];
            int code = status;
            if (etag != null && !etag.isEmpty() && etag.equals(lastIfNoneMatch)) {
              code = 304;
            } else {
              if (etag != null && !etag.isEmpty()) {
                exchange.getResponseHeaders().add("ETag", etag);
              }
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
      return new CdnFetcher(
          SECRET, key, url(), PROFILE, TestBackend.withMockRegistration(resolver));
    }

    String url() {
      return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    @Override
    public void close() {
      server.stop(0);
    }
  }
}
