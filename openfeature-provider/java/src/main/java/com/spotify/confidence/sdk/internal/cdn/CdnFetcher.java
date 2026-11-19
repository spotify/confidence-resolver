package com.spotify.confidence.sdk.internal.cdn;

import com.spotify.confidence.flags.wasm.v1.ClientModule;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.util.concurrent.atomic.AtomicReference;
import javax.crypto.Cipher;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * Polls one credential's compiled resolver module from the Confidence module CDN, using ETags for
 * conditional requests. Port of the Go provider's internal/cdn fetcher; that file is the reference
 * for semantics.
 */
public final class CdnFetcher {

  /** The production module CDN. */
  public static final String DEFAULT_BASE_URL =
      "https://confidence-resolver-module-cdn.spotifycdn.com";

  private static final int NONCE_SIZE = 12;
  private static final int TAG_BITS = 128;

  /** A decrypted module. The wasm embeds the client secret: never log it or write it to disk. */
  public record FetchedModule(byte[] wasm, String logToken, String etag) {}

  private final String url;
  private final SecretKey key;
  private final HttpClient client;
  private final AtomicReference<String> acceptedEtag = new AtomicReference<>();

  /** Validates a hex-encoded AES-256 key. The error never includes the key. */
  public static byte[] parseKey(String hexKey) {
    if (hexKey == null || hexKey.length() != 64) {
      throw new IllegalArgumentException("encryption key must be 64 hexadecimal characters");
    }
    byte[] key = new byte[32];
    for (int i = 0; i < 64; i++) {
      int hi = hexValue(hexKey.charAt(i));
      int lo = hexValue(hexKey.charAt(++i));
      if (hi < 0 || lo < 0) {
        throw new IllegalArgumentException("encryption key must be hexadecimal");
      }
      key[i / 2] = (byte) ((hi << 4) | lo);
    }
    return key;
  }

  private static int hexValue(char c) {
    if (c >= '0' && c <= '9') {
      return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
      return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
      return c - 'A' + 10;
    }
    return -1;
  }

  /**
   * Creates a fetcher for clientSecret. baseUrl may be empty (the production CDN); client may be
   * null (a default one).
   */
  public CdnFetcher(String clientSecret, byte[] key, String baseUrl, HttpClient client) {
    if (key == null || key.length != 32) {
      throw new IllegalArgumentException("encryption key must be 32 bytes (AES-256)");
    }
    this.key = new SecretKeySpec(key, "AES");
    if (baseUrl == null || baseUrl.isEmpty()) {
      baseUrl = DEFAULT_BASE_URL;
    }
    this.client = client == null ? HttpClient.newHttpClient() : client;
    String sum = sha256Hex(clientSecret);
    this.url = baseUrl + "/modules/" + sum + ".wasm.enc";
  }

  /**
   * Downloads and decrypts the module. Throws {@link NotModifiedException} when the module matches
   * the last accepted ETag. Call {@link #accept(String)} once the module is installed, so that a
   * module that fails to install is fetched again in full.
   */
  public FetchedModule fetch() {
    HttpRequest.Builder request = HttpRequest.newBuilder(URI.create(url)).GET();
    String etag = acceptedEtag.get();
    if (etag != null && !etag.isEmpty()) {
      request.header("If-None-Match", etag);
    }
    final HttpResponse<byte[]> response;
    try {
      response = client.send(request.build(), HttpResponse.BodyHandlers.ofByteArray());
    } catch (IOException e) {
      throw new CdnException("module CDN: " + e.getMessage(), e);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new CdnException("module CDN: interrupted", e);
    }
    int status = response.statusCode();
    if (status == 304) {
      throw new NotModifiedException();
    }
    if (status == 404) {
      throw new NotPublishedException();
    }
    if (status == 408 || status == 429 || status >= 500) {
      throw new CdnException("module CDN: " + status);
    }
    if (status != 200) {
      throw new TerminalCdnException("module CDN: " + status);
    }
    FetchedModule module = decrypt(response.body());
    return new FetchedModule(module.wasm(), module.logToken(), header(response, "ETag"));
  }

  /** Records etag as installed. Later fetches are conditional on it. */
  public void accept(String etag) {
    acceptedEtag.set(etag);
  }

  /**
   * Opens nonce(12) ‖ ciphertext ‖ tag(16) with empty associated data and decodes the ClientModule
   * inside. Artifact failures are terminal.
   */
  private FetchedModule decrypt(byte[] body) {
    if (body.length < NONCE_SIZE + TAG_BITS / 8) {
      throw new TerminalCdnException("module artifact too short");
    }
    byte[] plain;
    try {
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(Cipher.DECRYPT_MODE, key, new GCMParameterSpec(TAG_BITS, body, 0, NONCE_SIZE));
      plain = cipher.doFinal(body, NONCE_SIZE, body.length - NONCE_SIZE);
    } catch (GeneralSecurityException e) {
      throw new TerminalCdnException("module artifact failed to decrypt (wrong encryption key?)");
    }
    ClientModule artifact;
    try {
      artifact = ClientModule.parseFrom(plain);
    } catch (IOException e) {
      throw new TerminalCdnException("module artifact: " + e.getMessage(), e);
    }
    if (artifact.getModule().isEmpty() || artifact.getLogToken().isEmpty()) {
      throw new TerminalCdnException("module artifact is missing its module or log token");
    }
    return new FetchedModule(artifact.getModule().toByteArray(), artifact.getLogToken(), null);
  }

  private static String header(HttpResponse<byte[]> response, String name) {
    return response.headers().firstValue(name).orElse("");
  }

  private static String sha256Hex(String value) {
    try {
      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      byte[] sum = digest.digest(value.getBytes(StandardCharsets.UTF_8));
      StringBuilder hex = new StringBuilder(64);
      for (byte b : sum) {
        hex.append(String.format("%02x", b));
      }
      return hex.toString();
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("SHA-256 unavailable", e);
    }
  }
}
