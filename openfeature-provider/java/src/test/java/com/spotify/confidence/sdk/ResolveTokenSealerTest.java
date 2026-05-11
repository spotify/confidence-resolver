package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;

class ResolveTokenSealerTest {

  private final ResolveTokenSealer sealer = ResolveTokenSealer.create("test-key-do-not-use-in-prod");

  @Test
  void roundTripsATypicalResolveToken() {
    byte[] token = "abc.def.ghi-some-base64-ish-resolve-token==".getBytes(StandardCharsets.UTF_8);
    byte[] sealed = sealer.seal(token);
    assertThat(sealed).isNotEqualTo(token);
    assertThat(sealer.open(sealed)).isEqualTo(token);
  }

  @Test
  void roundTripsEmptyBytes() {
    byte[] sealed = sealer.seal(new byte[0]);
    assertThat(sealer.open(sealed)).isEmpty();
  }

  @Test
  void producesDifferentCiphertextEachCall() {
    byte[] token = "same-input".getBytes(StandardCharsets.UTF_8);
    assertThat(sealer.seal(token)).isNotEqualTo(sealer.seal(token));
  }

  @Test
  void rejectsTamperedCiphertext() {
    byte[] sealed = sealer.seal("something".getBytes(StandardCharsets.UTF_8));
    sealed[sealed.length - 1] ^= 0x01;
    byte[] tampered = sealed;
    assertThatThrownBy(() -> sealer.open(tampered))
        .isInstanceOf(IllegalArgumentException.class);
  }

  @Test
  void rejectsTooShortHandle() {
    assertThatThrownBy(() -> sealer.open(new byte[] {1, 2, 3, 4}))
        .isInstanceOf(IllegalArgumentException.class)
        .hasMessage("Invalid Confidence handle");
  }

  @Test
  void rejectsHandleSealedWithDifferentKey() {
    byte[] sealed = sealer.seal("payload".getBytes(StandardCharsets.UTF_8));
    ResolveTokenSealer other = ResolveTokenSealer.create("a-different-key");
    assertThatThrownBy(() -> other.open(sealed))
        .isInstanceOf(IllegalArgumentException.class);
  }
}
