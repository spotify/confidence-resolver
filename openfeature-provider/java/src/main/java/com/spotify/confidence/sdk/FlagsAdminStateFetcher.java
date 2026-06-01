package com.spotify.confidence.sdk;

import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicReference;
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
  // ETag for conditional GETs of resolver state
  private final AtomicReference<String> etagHolder = new AtomicReference<>();
  private final AtomicReference<byte[]> rawResolverStateHolder =
      new AtomicReference<>(
          com.spotify.confidence.sdk.flags.admin.v1.ResolverState.newBuilder()
              .build()
              .toByteArray());
  private final AtomicReference<byte[]> rawCdnBytesHolder = new AtomicReference<>();
  private String accountId = "";

  public FlagsAdminStateFetcher(String clientSecret, HttpClientFactory httpClientFactory) {
    this(clientSecret, httpClientFactory, null);
  }

  public FlagsAdminStateFetcher(
      String clientSecret, HttpClientFactory httpClientFactory, String encryptionKey) {
    this.clientSecret = clientSecret;
    this.httpClientFactory = httpClientFactory;
    this.encryptionKey = encryptionKey;
  }

  public AtomicReference<byte[]> rawStateHolder() {
    return rawResolverStateHolder;
  }

  public byte[] rawCdnBytes() {
    return rawCdnBytesHolder.get();
  }

  @Override
  public byte[] provide() {
    final byte[] cdnBytes = rawCdnBytesHolder.get();
    if (cdnBytes != null) {
      return cdnBytes;
    }
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
    // Build CDN URL using SHA256 hash of client secret
    final var cdnUrl = CDN_BASE_URL + sha256Hex(clientSecret);
    try {
      final HttpURLConnection conn = httpClientFactory.create(cdnUrl);
      final String previousEtag = etagHolder.get();
      if (previousEtag != null) {
        conn.setRequestProperty("if-none-match", previousEtag);
      }
      if (conn.getResponseCode() == 304) {
        // Not modified
        return;
      }
      final String etag = conn.getHeaderField("etag");
      final boolean encrypted = "true".equals(conn.getHeaderField("x-goog-meta-encrypted"));
      try (final InputStream stream = conn.getInputStream()) {
        final byte[] bytes = stream.readAllBytes();

        if (encrypted) {
          if (encryptionKey == null || encryptionKey.isEmpty()) {
            throw new RuntimeException(
                "Resolver state is encrypted but no encryptionKey was provided. "
                    + "Set the encryption key for this client credential.");
          }
          rawCdnBytesHolder.set(bytes);
        } else {
          final var stateRequest =
              com.spotify.confidence.sdk.wasm.Messages.SetResolverStateRequest.parseFrom(bytes);
          this.accountId = stateRequest.getAccountId();
          rawResolverStateHolder.set(stateRequest.getState().toByteArray());
          rawCdnBytesHolder.set(null);
        }
        etagHolder.set(etag);
      }
      logger.info(
          "Loaded resolver state (encrypted={}, etag={})", encrypted, etag);
    } catch (IOException e) {
      throw new RuntimeException(e);
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
