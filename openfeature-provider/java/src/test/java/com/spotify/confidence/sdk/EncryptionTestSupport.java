package com.spotify.confidence.sdk;

import java.nio.ByteBuffer;
import java.security.SecureRandom;
import java.util.HexFormat;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

final class EncryptionTestSupport {
  static final String KEY = "00".repeat(32);
  private static final SecureRandom RANDOM = new SecureRandom();

  static byte[] encrypt(byte[] plaintext) {
    return encrypt(plaintext, KEY);
  }

  static byte[] encrypt(byte[] plaintext, String key) {
    try {
      byte[] nonce = new byte[12];
      RANDOM.nextBytes(nonce);
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(
          Cipher.ENCRYPT_MODE,
          new SecretKeySpec(HexFormat.of().parseHex(key), "AES"),
          new GCMParameterSpec(128, nonce));
      byte[] ciphertext = cipher.doFinal(plaintext);
      return ByteBuffer.allocate(nonce.length + ciphertext.length)
          .put(nonce)
          .put(ciphertext)
          .array();
    } catch (Exception e) {
      throw new AssertionError(e);
    }
  }
}
