package com.spotify.confidence.sdk;

import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.security.SecureRandom;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * Seals and opens resolve tokens using AES-256-GCM. The resolve token carries the full evaluation
 * context and the resolved variants — this class ensures the client only ever sees an opaque,
 * encrypted handle that it round-trips through the apply endpoint.
 *
 * <p>Usage:
 *
 * <pre>{@code
 * // Generate a key: openssl rand -hex 32
 * ResolveTokenSealer sealer = ResolveTokenSealer.create("my-secret-key");
 * FlagResolverService service = new FlagResolverService<>(provider, sealer);
 * }</pre>
 */
@Experimental
public final class ResolveTokenSealer {
  private static final String ALGO = "AES/GCM/NoPadding";
  private static final int IV_LEN = 12;
  private static final int TAG_BITS = 128;
  private static final int TAG_LEN = TAG_BITS / 8;
  private static final SecureRandom RANDOM = new SecureRandom();

  private final byte[] keyBytes;

  private ResolveTokenSealer(byte[] keyBytes) {
    this.keyBytes = keyBytes;
  }

  /**
   * Creates a sealer from a raw key string. The key is derived via SHA-256 to produce a 256-bit AES
   * key. Generate one with: {@code openssl rand -hex 32}
   *
   * @param rawKey the secret key (any length; will be SHA-256 hashed)
   * @return a new sealer
   */
  public static ResolveTokenSealer create(String rawKey) {
    try {
      byte[] derived =
          MessageDigest.getInstance("SHA-256").digest(rawKey.getBytes(StandardCharsets.UTF_8));
      return new ResolveTokenSealer(derived);
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("SHA-256 not available", e);
    }
  }

  /**
   * Encrypts a resolve token. Each call produces a different ciphertext (random IV).
   *
   * @param plaintext the raw resolve token bytes
   * @return sealed bytes: {@code IV (12) || GCM tag (16) || ciphertext}
   */
  byte[] seal(byte[] plaintext) {
    try {
      byte[] iv = new byte[IV_LEN];
      RANDOM.nextBytes(iv);
      Cipher cipher = Cipher.getInstance(ALGO);
      cipher.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(keyBytes, "AES"), gcmSpec(iv));
      byte[] ciphertextAndTag = cipher.doFinal(plaintext);
      // Java appends the tag to ciphertext. Rearrange to: IV || tag || ciphertext
      // to match the JS SDK wire format.
      int ciphertextLen = ciphertextAndTag.length - TAG_LEN;
      byte[] result = new byte[IV_LEN + TAG_LEN + ciphertextLen];
      System.arraycopy(iv, 0, result, 0, IV_LEN);
      System.arraycopy(ciphertextAndTag, ciphertextLen, result, IV_LEN, TAG_LEN);
      System.arraycopy(ciphertextAndTag, 0, result, IV_LEN + TAG_LEN, ciphertextLen);
      return result;
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("AES-GCM seal failed", e);
    }
  }

  /**
   * Decrypts a sealed resolve token.
   *
   * @param sealed the sealed bytes (as produced by {@link #seal})
   * @return the original plaintext bytes
   * @throws IllegalArgumentException if the handle is too short or tampered with
   */
  byte[] open(byte[] sealed) {
    if (sealed.length < IV_LEN + TAG_LEN) {
      throw new IllegalArgumentException("Invalid Confidence handle");
    }
    try {
      byte[] iv = new byte[IV_LEN];
      System.arraycopy(sealed, 0, iv, 0, IV_LEN);
      byte[] tag = new byte[TAG_LEN];
      System.arraycopy(sealed, IV_LEN, tag, 0, TAG_LEN);
      int ciphertextLen = sealed.length - IV_LEN - TAG_LEN;
      // Reassemble to Java's expected format: ciphertext || tag
      byte[] ciphertextAndTag = new byte[ciphertextLen + TAG_LEN];
      System.arraycopy(sealed, IV_LEN + TAG_LEN, ciphertextAndTag, 0, ciphertextLen);
      System.arraycopy(tag, 0, ciphertextAndTag, ciphertextLen, TAG_LEN);
      Cipher cipher = Cipher.getInstance(ALGO);
      cipher.init(Cipher.DECRYPT_MODE, new SecretKeySpec(keyBytes, "AES"), gcmSpec(iv));
      return cipher.doFinal(ciphertextAndTag);
    } catch (GeneralSecurityException e) {
      throw new IllegalArgumentException("Invalid Confidence handle", e);
    }
  }

  private static GCMParameterSpec gcmSpec(byte[] iv) {
    return new GCMParameterSpec(TAG_BITS, iv);
  }
}
