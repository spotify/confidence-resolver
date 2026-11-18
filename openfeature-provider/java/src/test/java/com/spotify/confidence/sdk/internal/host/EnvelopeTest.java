package com.spotify.confidence.sdk.internal.host;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.google.protobuf.InvalidProtocolBufferException;
import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.flags.wasm.v1.ResolveProcessResponse;
import com.spotify.confidence.flags.wasm.v1.ResolveProcessResponse.Error.Code;
import org.junit.jupiter.api.Test;

/**
 * Pins the process-envelope wire behavior the resolver relies on (the Go host tests its hand-rolled
 * decoder; here the envelope is the generated protobuf parse).
 */
class EnvelopeTest {

  @Test
  void decodesAResolvedEnvelope() throws InvalidProtocolBufferException {
    ResolveProcessResponse resolved = ResolveProcessResponse.parseFrom(resolvedBytes());
    assertThat(resolved.getResultCase()).isEqualTo(ResolveProcessResponse.ResultCase.RESOLVED);
    assertThat(resolved.getResolved().getClientIndex()).isEqualTo(3);
    assertThat(resolved.getResolved().getMaterializationToWriteCount()).isEqualTo(1);
  }

  @Test
  void decodesASuspendedEnvelope() throws InvalidProtocolBufferException {
    ResolveProcessResponse suspended = ResolveProcessResponse.parseFrom(suspendedBytes());
    assertThat(suspended.getResultCase()).isEqualTo(ResolveProcessResponse.ResultCase.SUSPENDED);
    assertThat(suspended.getSuspended().getProcessId()).isEqualTo(7);
    assertThat(suspended.getSuspended().getMaterializationToReadCount()).isEqualTo(1);
  }

  @Test
  void decodesAnErrorEnvelope() throws InvalidProtocolBufferException {
    ResolveProcessResponse rejected =
        ResolveProcessResponse.parseFrom(
            ResolveProcessResponse.newBuilder()
                .setError(
                    ResolveProcessResponse.Error.newBuilder()
                        .setCode(Code.INVALID_ARGUMENT)
                        .setMessage("bad")
                        .build())
                .build()
                .toByteArray());
    assertThat(rejected.getResultCase()).isEqualTo(ResolveProcessResponse.ResultCase.ERROR);
    assertThat(rejected.getError().getCode()).isEqualTo(Code.INVALID_ARGUMENT);
    assertThat(rejected.getError().getMessage()).isEqualTo("bad");
  }

  @Test
  void anEmptyEnvelopeIsNoResult() throws InvalidProtocolBufferException {
    // The resolver treats no result kind as "empty process response".
    assertThat(ResolveProcessResponse.parseFrom(new byte[0]).getResultCase())
        .isEqualTo(ResolveProcessResponse.ResultCase.RESULT_NOT_SET);
  }

  @Test
  void malformedEnvelopesAreRejected() {
    for (byte[] bytes : new byte[][] {{(byte) 0x0a, 0x05, 0x01}, {(byte) 0xff}}) {
      assertThatThrownBy(() -> ResolveProcessResponse.parseFrom(bytes))
          .isInstanceOf(InvalidProtocolBufferException.class);
    }
  }

  private static byte[] resolvedBytes() {
    return ResolveProcessResponse.newBuilder()
        .setResolved(
            ResolveProcessResponse.Resolved.newBuilder()
                .setClientIndex(3)
                .addMaterializationToWrite(
                    Record.newBuilder()
                        .setUnit("u1")
                        .setMaterialization("materializedSegments/sticky")
                        .build()))
        .build()
        .toByteArray();
  }

  private static byte[] suspendedBytes() {
    return ResolveProcessResponse.newBuilder()
        .setSuspended(
            ResolveProcessResponse.Suspended.newBuilder()
                .setProcessId(7)
                .addMaterializationToRead(
                    Record.newBuilder()
                        .setUnit("u1")
                        .setMaterialization("materializedSegments/sticky")
                        .build()))
        .build()
        .toByteArray();
  }
}
