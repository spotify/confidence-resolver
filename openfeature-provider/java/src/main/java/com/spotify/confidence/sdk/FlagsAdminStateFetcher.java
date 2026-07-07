package com.spotify.confidence.sdk;

import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicReference;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Fetches and caches account state from the Confidence CDN.
 *
 * <p>This implementation fetches state directly from the CDN using the client secret, using ETags
 * for conditional GETs to minimize bandwidth.
 *
 * <p>Thread-safe implementation using atomic references for concurrent access.
 */
class FlagsAdminStateFetcher implements AccountStateProvider {

  private static final Logger logger = LoggerFactory.getLogger(FlagsAdminStateFetcher.class);
  private static final String CDN_BASE_URL =
      "https://confidence-resolver-state-cdn.spotifycdn.com/";

  private final String clientSecret;
  private final String encryptionKey;
  private final HttpClientFactory httpClientFactory;
  private final AtomicReference<String> etagHolder = new AtomicReference<>();
  private final AtomicReference<byte[]> rawResolverStateHolder =
      new AtomicReference<>(
          com.spotify.confidence.sdk.flags.admin.v1.ResolverState.newBuilder()
              .build()
              .toByteArray());
  private String accountId = "";

  public FlagsAdminStateFetcher(
      String clientSecret, HttpClientFactory httpClientFactory, String encryptionKey) {
    this.clientSecret = clientSecret;
    this.httpClientFactory = httpClientFactory;
    this.encryptionKey = encryptionKey;
  }

  @Override
  public byte[] provide() {
    return rawResolverStateHolder.get();
  }

  @Override
  public String accountId() {
    return accountId;
  }

  @Override
  public void reload() {
    try {
      fetchAndUpdateStateIfChanged();
    } catch (Exception e) {
      logger.warn("Failed to reload, ignoring reload", e);
    }
  }

  private void fetchAndUpdateStateIfChanged() {
    final String hash = sha256Hex(clientSecret);
    final var cdnUrl = CDN_BASE_URL + hash + (encryptionKey != null ? ".enc" : "");
    try {
      final HttpURLConnection conn = httpClientFactory.create(cdnUrl);
      final String previousEtag = etagHolder.get();
      if (previousEtag != null) {
        conn.setRequestProperty("if-none-match", previousEtag);
      }
      if (conn.getResponseCode() == 304) {
        return;
      }
      final String etag = conn.getHeaderField("etag");
      try (final InputStream stream = conn.getInputStream()) {
        byte[] bytes = stream.readAllBytes();

        if (encryptionKey != null) {
          bytes = decryptAesGcm(bytes, encryptionKey);
        }

        final var stateRequest =
            com.spotify.confidence.sdk.wasm.Messages.SetResolverStateRequest.parseFrom(bytes);
        this.accountId = stateRequest.getAccountId();
        rawResolverStateHolder.set(stateRequest.getState().toByteArray());
        etagHolder.set(etag);
      }
      logger.info("Loaded resolver state for account={}, etag={}", accountId, etag);
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  private static byte[] decryptAesGcm(byte[] data, String hexKey) {
    try {
      final byte[] key = java.util.HexFormat.of().parseHex(hexKey);
      final int nonceLen = 12;
      if (data.length < nonceLen) {
        throw new IllegalArgumentException("Encrypted state too short (missing nonce)");
      }
      final var cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(
          Cipher.DECRYPT_MODE,
          new SecretKeySpec(key, "AES"),
          new GCMParameterSpec(128, data, 0, nonceLen));
      return cipher.doFinal(data, nonceLen, data.length - nonceLen);
    } catch (Exception e) {
      throw new RuntimeException("Failed to decrypt resolver state", e);
    }
  }

  private static String sha256Hex(String input) {
    try {
      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      byte[] hash = digest.digest(input.getBytes(StandardCharsets.UTF_8));
      StringBuilder hexString = new StringBuilder();
      for (byte b : hash) {
        String hex = Integer.toHexString(0xff & b);
        if (hex.length() == 1) hexString.append('0');
        hexString.append(hex);
      }
      return hexString.toString();
    } catch (NoSuchAlgorithmException e) {
      throw new RuntimeException("SHA-256 algorithm not available", e);
    }
  }
}
