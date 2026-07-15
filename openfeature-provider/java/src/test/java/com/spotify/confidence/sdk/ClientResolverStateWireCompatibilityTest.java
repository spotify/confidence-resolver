package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.UnknownFieldSet;
import com.spotify.confidence.sdk.wasm.Messages;
import org.junit.jupiter.api.Test;

/**
 * Verifies that ClientResolverState (from admin) and SetResolverStateRequest (WASM) remain wire
 * compatible. The CDN serves ClientResolverState bytes, and the SDK parses them as
 * SetResolverStateRequest. Fields added to ClientResolverState must not collide with
 * SetResolverStateRequest tags.
 *
 * <p>SetResolverStateRequest uses: tag 1 (state), tag 2 (account_id), tag 3 (Sdk).
 */
class ClientResolverStateWireCompatibilityTest {

  private static final byte[] MINIMAL_STATE = new byte[] {0x08, 0x01};
  private static final String ACCOUNT = "test-account";

  /**
   * Packed repeated int32 at tag 3 uses wire type 2 (length-delimited) — same wire type as the Sdk
   * message at tag 3 in SetResolverStateRequest. The parser interprets the packed int32 bytes as an
   * Sdk message body, producing a corrupted Sdk with garbage fields.
   */
  @Test
  void tag3_packed_int32_corrupts_sdk_field() throws Exception {
    // Packed encoding for repeated int32 [1] = one byte: 0x01
    final byte[] packedInt = new byte[] {0x01};
    final var proto =
        UnknownFieldSet.newBuilder()
            .addField(
                1,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFrom(MINIMAL_STATE))
                    .build())
            .addField(
                2,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFromUtf8(ACCOUNT))
                    .build())
            .addField(
                3,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFrom(packedInt))
                    .build())
            .build();

    // Tag 3 collision: packed int32 bytes are misinterpreted as Sdk message body → parse fails
    assertThatThrownBy(() -> Messages.SetResolverStateRequest.parseFrom(proto.toByteArray()))
        .isInstanceOf(InvalidProtocolBufferException.class);
  }

  /** Tag 4 is unused in SetResolverStateRequest — protobuf silently skips it. */
  @Test
  void tag4_packed_int32_is_safely_skipped() throws Exception {
    final byte[] packedInt = new byte[] {0x01};
    final var proto =
        UnknownFieldSet.newBuilder()
            .addField(
                1,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFrom(MINIMAL_STATE))
                    .build())
            .addField(
                2,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFromUtf8(ACCOUNT))
                    .build())
            .addField(
                4,
                UnknownFieldSet.Field.newBuilder()
                    .addLengthDelimited(ByteString.copyFrom(packedInt))
                    .build())
            .build();

    final var parsed = Messages.SetResolverStateRequest.parseFrom(proto.toByteArray());
    assertThat(parsed.getAccountId()).isEqualTo(ACCOUNT);
    assertThat(parsed.getState().toByteArray()).isEqualTo(MINIMAL_STATE);
    assertThat(parsed.hasSdk()).isFalse();
  }
}
