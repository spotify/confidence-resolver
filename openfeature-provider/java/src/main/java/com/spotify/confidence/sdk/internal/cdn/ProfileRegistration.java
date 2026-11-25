package com.spotify.confidence.sdk.internal.cdn;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.google.protobuf.util.Durations;
import com.google.protobuf.util.JsonFormat;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.text.ParseException;
import java.time.Duration;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

final class ProfileRegistration {

  private static final String DEFAULT_RESOLVER_BASE_URL = "https://resolver.confidence.dev";
  private static final Duration REQUEST_TIMEOUT = Duration.ofSeconds(10);

  private final String clientSecret;
  private final String url;
  private final HttpClient client;
  private final String requestBody;
  private HttpClient registrationClient;

  ProfileRegistration(String clientSecret, String baseUrl, String profile, HttpClient client) {
    this.clientSecret = clientSecret;
    String base = baseUrl == null || baseUrl.isEmpty() ? DEFAULT_RESOLVER_BASE_URL : baseUrl;
    this.url = base.replaceFirst("/+$", "") + "/v1/profile:register";
    this.client = client;
    this.requestBody = "{\"profile\":\"" + profile + "\"}";
  }

  long register(Duration remaining) {
    Duration timeout = remaining.compareTo(REQUEST_TIMEOUT) < 0 ? remaining : REQUEST_TIMEOUT;
    final HttpResponse<String> response;
    try {
      HttpRequest request =
          HttpRequest.newBuilder(URI.create(url))
              .timeout(timeout)
              .header("Authorization", "ClientSecret " + clientSecret)
              .header("Content-Type", "application/json")
              .POST(HttpRequest.BodyPublishers.ofString(requestBody))
              .build();
      if (registrationClient == null) {
        registrationClient = withoutRedirects(client);
      }
      var pending =
          registrationClient.sendAsync(
              request,
              info ->
                  info.statusCode() == 200
                      ? HttpResponse.BodySubscribers.ofString(StandardCharsets.UTF_8)
                      : HttpResponse.BodySubscribers.replacing(""));
      try {
        response = pending.get(timeout.toNanos(), TimeUnit.NANOSECONDS);
      } catch (InterruptedException | TimeoutException e) {
        pending.cancel(true);
        throw e;
      }
    } catch (IllegalArgumentException e) {
      // Request-building errors can include the Authorization header value.
      throw new TerminalCdnException("invalid module profile registration request");
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new CdnException("module profile registration interrupted");
    } catch (TimeoutException | ExecutionException e) {
      throw new CdnException("module profile registration request failed");
    }
    int status = response.statusCode();
    if (status == 408 || status == 429 || status >= 500) {
      throw new CdnException("module profile registration: " + status);
    }
    if (status != 200) {
      throw new TerminalCdnException("module profile registration: " + status);
    }
    try {
      Struct.Builder ack = Struct.newBuilder();
      JsonFormat.parser().merge(response.body(), ack);
      Value delay = ack.getFieldsOrDefault("cdnRetryDelay", Value.getDefaultInstance());
      if (delay.getKindCase() == Value.KindCase.STRING_VALUE) {
        long nanos = Durations.toNanos(Durations.parse(delay.getStringValue()));
        if (nanos >= 0) {
          return nanos;
        }
      }
    } catch (IOException | ParseException | IllegalArgumentException | ArithmeticException e) {
      // A malformed or unrepresentable delay must never cause early CDN polling.
    }
    throw new TerminalCdnException("invalid module profile registration acknowledgment");
  }

  private static HttpClient withoutRedirects(HttpClient client) {
    if (client.followRedirects() == HttpClient.Redirect.NEVER) {
      return client;
    }
    // Redirect policy is per client, so preserve transport settings on a separate client.
    HttpClient.Builder builder =
        HttpClient.newBuilder()
            .followRedirects(HttpClient.Redirect.NEVER)
            .version(client.version())
            .sslContext(client.sslContext())
            .sslParameters(client.sslParameters());
    client.connectTimeout().ifPresent(builder::connectTimeout);
    client.proxy().ifPresent(builder::proxy);
    client.executor().ifPresent(builder::executor);
    return builder.build();
  }
}
