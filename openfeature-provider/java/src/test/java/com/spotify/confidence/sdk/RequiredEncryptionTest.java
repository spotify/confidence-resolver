package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;
import static org.mockito.Mockito.*;

import com.google.protobuf.ByteString;
import com.spotify.confidence.sdk.flags.admin.v1.ClientResolverState;
import java.io.ByteArrayInputStream;
import java.net.HttpURLConnection;
import org.junit.jupiter.api.Test;

class RequiredEncryptionTest {
  @Test
  void rejectsInvalidKeysBeforeCreatingTransports() {
    ChannelFactory channels = mock(ChannelFactory.class);
    HttpClientFactory http = mock(HttpClientFactory.class);
    for (String key :
        new String[] {
          null, "", " ", "00".repeat(31), "00".repeat(33), "gg".repeat(32), "00".repeat(32) + "\n"
        }) {
      LocalProviderConfig config =
          LocalProviderConfig.builder()
              .channelFactory(channels)
              .httpClientFactory(http)
              .useRemoteMaterializationStore(true)
              .build();
      assertThatThrownBy(() -> new OpenFeatureLocalResolveProvider(config, "secret", key))
          .isInstanceOf(IllegalArgumentException.class)
          .hasMessageContaining("64 hexadecimal");
      assertThatThrownBy(
              () ->
                  new OpenFeatureLocalResolveProvider(
                      config, "secret", key, new UnsupportedMaterializationStore()))
          .isInstanceOf(IllegalArgumentException.class);
      assertThatThrownBy(() -> new OpenFeatureLocalResolveProvider("secret", key))
          .isInstanceOf(IllegalArgumentException.class);
      assertThatThrownBy(
              () ->
                  new OpenFeatureLocalResolveProvider(
                      "secret", key, new UnsupportedMaterializationStore()))
          .isInstanceOf(IllegalArgumentException.class);
      assertThatThrownBy(() -> new FlagsAdminStateFetcher("secret", http, key))
          .isInstanceOf(IllegalArgumentException.class);
    }
    verifyNoInteractions(channels, http);
    assertThat(FlagsAdminStateFetcher.validateEncryptionKey("ab".repeat(32)))
        .isEqualTo("ab".repeat(32));
    assertThat(FlagsAdminStateFetcher.validateEncryptionKey("AB".repeat(32)))
        .isEqualTo("AB".repeat(32));
  }

  @Test
  void preservesStateAndEtagAfterFailedDecryption() throws Exception {
    byte[] plaintext =
        ClientResolverState.newBuilder()
            .setState(ByteString.copyFromUtf8("state"))
            .setAccount("account")
            .build()
            .toByteArray();
    byte[] encrypted = EncryptionTestSupport.encrypt(plaintext);
    byte[] tampered = encrypted.clone();
    tampered[tampered.length - 1] ^= 1;
    for (byte[] bad :
        new byte[][] {
          plaintext,
          new byte[5],
          tampered,
          EncryptionTestSupport.encrypt(plaintext, "01".repeat(32))
        }) {
      HttpClientFactory http = mock(HttpClientFactory.class);
      HttpURLConnection first = response(encrypted, "good");
      HttpURLConnection failed = response(bad, "bad");
      HttpURLConnection retry = response(encrypted, "recovered");
      HttpURLConnection unchanged = mock(HttpURLConnection.class);
      when(unchanged.getResponseCode()).thenReturn(304);
      when(http.create(anyString())).thenReturn(first, failed, retry, unchanged);
      FlagsAdminStateFetcher fetcher =
          new FlagsAdminStateFetcher("secret", http, EncryptionTestSupport.KEY);
      fetcher.reload();
      fetcher.reload();
      assertThat(fetcher.accountId()).isEqualTo("account");
      assertThat(fetcher.provide())
          .isEqualTo("state".getBytes(java.nio.charset.StandardCharsets.UTF_8));
      fetcher.reload();
      verify(retry).setRequestProperty("if-none-match", "good");
      fetcher.reload();
      verify(unchanged).setRequestProperty("if-none-match", "recovered");
      verify(http, times(4)).create(endsWith(".enc"));
    }
  }

  private static HttpURLConnection response(byte[] bytes, String etag) throws Exception {
    HttpURLConnection conn = mock(HttpURLConnection.class);
    when(conn.getResponseCode()).thenReturn(200);
    when(conn.getHeaderField("etag")).thenReturn(etag);
    when(conn.getInputStream()).thenReturn(new ByteArrayInputStream(bytes));
    return conn;
  }
}
