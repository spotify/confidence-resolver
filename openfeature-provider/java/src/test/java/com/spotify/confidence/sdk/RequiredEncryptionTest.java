package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * The provider rejects malformed encryption keys at construction, before any transport is created,
 * as the old stack's RequiredEncryptionTest; that file is the reference for the cases.
 */
class RequiredEncryptionTest {

  private static final String VALID = "0f".repeat(32);

  @Test
  void rejectsInvalidKeysBeforeCreatingTransports() {
    String[] bad = {
      null,
      "",
      "   ",
      "0f".repeat(31), // 62 chars
      "0f".repeat(32) + "0", // 65 chars
      "zz".repeat(32), // not hex
      VALID + "\n", // trailing newline
      VALID.substring(0, 63),
    };
    for (String key : bad) {
      assertThrows(
          IllegalArgumentException.class,
          () -> new OpenFeatureLocalResolveProvider("secret", key),
          "key " + (key == null ? "null" : "len " + key.length()));
      assertThrows(
          IllegalArgumentException.class,
          () ->
              new OpenFeatureLocalResolveProvider(
                  LocalProviderConfig.builder().build(), "secret", key));
      assertThrows(
          IllegalArgumentException.class,
          () ->
              new OpenFeatureLocalResolveProvider(
                  "secret", key, new InMemoryMaterializationStoreExample()));
      assertThrows(
          IllegalArgumentException.class,
          () ->
              new OpenFeatureLocalResolveProvider(
                  LocalProviderConfig.builder().build(),
                  "secret",
                  key,
                  new InMemoryMaterializationStoreExample()));
    }
  }

  @Test
  void rejectsBlankSecrets() {
    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider(null, VALID));
    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider("", VALID));
    assertThrows(
        IllegalArgumentException.class, () -> new OpenFeatureLocalResolveProvider("  ", VALID));
  }

  @Test
  void acceptsSixtyFourHexCharacters() {
    assertDoesNotThrow(
        () -> {
          OpenFeatureLocalResolveProvider p = new OpenFeatureLocalResolveProvider("secret", VALID);
          p.shutdown();
        });
    assertDoesNotThrow(
        () -> {
          OpenFeatureLocalResolveProvider p =
              new OpenFeatureLocalResolveProvider("secret", VALID.toUpperCase());
          p.shutdown();
        });
  }

  @Test
  void messageNamesTheRequirement() {
    IllegalArgumentException e =
        assertThrows(
            IllegalArgumentException.class,
            () -> new OpenFeatureLocalResolveProvider("secret", "short"));
    assertTrue(e.getMessage().contains("64 hexadecimal"), e.getMessage());
    assertTrue(!e.getMessage().contains(VALID), "the error must not leak a valid key");
  }
}
