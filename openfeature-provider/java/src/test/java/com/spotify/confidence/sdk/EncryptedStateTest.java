package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.google.protobuf.util.Structs;
import com.google.protobuf.util.Values;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveReason;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.HexFormat;
import java.util.List;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class EncryptedStateTest {

  private static final String CLIENT_SECRET = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF";

  private WasmLocalResolver resolver;
  private byte[] encryptedState;
  private byte[] encryptionKey;

  @BeforeEach
  void setUp() throws IOException {
    resolver = new WasmLocalResolver(request -> {});
    encryptedState = getClass().getResourceAsStream("/resolver_state_encrypted.pb").readAllBytes();
    encryptionKey =
        HexFormat.of()
            .parseHex(
                new String(
                        getClass().getResourceAsStream("/encryption_key_test.hex").readAllBytes(),
                        StandardCharsets.UTF_8)
                    .trim());
  }

  @Test
  void shouldResolveAfterSettingEncryptedState() {
    resolver.setEncryptedResolverState(encryptedState, encryptionKey, null);

    final var request =
        ResolveProcessRequest.newBuilder()
            .setDeferredMaterializations(
                ResolveFlagsRequest.newBuilder()
                    .addAllFlags(List.of("flags/tutorial-feature"))
                    .setClientSecret(CLIENT_SECRET)
                    .setEvaluationContext(
                        Structs.of(
                            "targeting_key",
                            Values.of("tutorial_visitor"),
                            "visitor_id",
                            Values.of("tutorial_visitor")))
                    .setApply(true)
                    .build())
            .build();

    final ResolveProcessResponse response =
        resolver.resolveProcess(request).toCompletableFuture().join();
    assertThat(response.hasResolved()).isTrue();
    assertThat(response.getResolved().getResponse().getResolvedFlagsCount()).isEqualTo(1);
    final var resolvedFlag = response.getResolved().getResponse().getResolvedFlags(0);
    assertThat(resolvedFlag.getReason())
        .withFailMessage(
            "Expected MATCH but got %s (flag=%s, variant=%s)",
            resolvedFlag.getReason(), resolvedFlag.getFlag(), resolvedFlag.getVariant())
        .isEqualTo(ResolveReason.RESOLVE_REASON_MATCH);
  }

  @Test
  void shouldRejectWrongEncryptionKey() {
    final byte[] wrongKey = new byte[32];
    assertThrows(
        RuntimeException.class,
        () -> resolver.setEncryptedResolverState(encryptedState, wrongKey, null));
  }
}
