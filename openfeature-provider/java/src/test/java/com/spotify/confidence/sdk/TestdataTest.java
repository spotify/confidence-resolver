package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import com.google.protobuf.ByteString;
import com.spotify.confidence.flags.admin.v1.Segment;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.types.v1.FlagSchema;
import com.spotify.confidence.flags.wasm.v1.ClientModule;
import com.spotify.confidence.flags.wasm.v1.WriteFlagLogsRequest;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;

/**
 * The shared provider test data, as described by openfeature-provider/testdata/README.md: a state
 * and the module compiled from it, read by relative path (the Docker stages keep the repository
 * layout, as the Go provider's do).
 */
class TestdataTest {

  static final Path TESTDATA = Path.of("..", "testdata");

  @Test
  void moduleFixtureIsLoadable() throws Exception {
    Path module = TESTDATA.resolve("modules/basic.wasm");
    assertThat(module).exists();
    byte[] bytes = Files.readAllBytes(module);
    // The state README documents a threaded state-to-wasm module with the evaluate export.
    assertThat(bytes.length).isGreaterThan(100_000);
    assertThat(bytes[0]).isZero();
    assertThat(bytes[1]).isEqualTo((byte) 'a');
    assertThat(bytes[2]).isEqualTo((byte) 's');
    assertThat(bytes[3]).isEqualTo((byte) 'm');
  }

  @Test
  void stateFixtureCarriesTheDocumentedFlags() throws Exception {
    // The state's message is state-to-wasm's own (not part of the provider protos, which only
    // carry the module API); assert the shapes the module fixture covers per its README.
    String state = Files.readString(TESTDATA.resolve("states/basic.json"));
    assertThat(state).contains("flags/plain");
    assertThat(state).contains("flags/sticky");
    assertThat(state).contains("materializedSegments/sticky");
  }

  @Test
  void clientModuleRoundTrips() throws Exception {
    // The CDN artifact: encrypted on the wire, decrypted by the fetcher into this message.
    byte[] module = Files.readAllBytes(TESTDATA.resolve("modules/basic.wasm"));
    ClientModule original =
        ClientModule.newBuilder()
            .setModule(ByteString.copyFrom(module))
            .setLogToken("not-a-real-token")
            .build();
    ClientModule parsed = ClientModule.parseFrom(original.toByteArray());
    assertThat(parsed.getModule().toByteArray()).isEqualTo(module);
    assertThat(parsed.getLogToken()).isEqualTo("not-a-real-token");
  }

  @Test
  void generatedPackagesAreOnTheClasspath() {
    // One type per package the provider protos generate; pins the include set in the pom.
    assertThat(WriteFlagLogsRequest.getDefaultInstance()).isNotNull();
    assertThat(ResolveFlagsRequest.getDefaultInstance()).isNotNull();
    assertThat(FlagSchema.getDefaultInstance()).isNotNull();
    assertThat(Segment.getDefaultInstance()).isNotNull();
  }
}
