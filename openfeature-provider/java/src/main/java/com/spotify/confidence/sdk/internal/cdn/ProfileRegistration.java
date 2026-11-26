package com.spotify.confidence.sdk.internal.cdn;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

final class ProfileRegistration {

  private static final URI REGISTRATION_URL =
      URI.create("https://resolver.confidence.dev/v1/profile:register");
  private static final Duration REQUEST_TIMEOUT = Duration.ofSeconds(10);

  private final String clientSecret;
  private final HttpClient client;
  private final String requestBody;
  private HttpClient registrationClient;

  ProfileRegistration(String clientSecret, String profile, HttpClient client) {
    this.clientSecret = clientSecret;
    this.client = client;
    this.requestBody = "{\"profile\":\"" + profile + "\"}";
  }

  void register(Duration remaining) {
    Duration timeout = remaining.compareTo(REQUEST_TIMEOUT) < 0 ? remaining : REQUEST_TIMEOUT;
    final HttpResponse<Void> response;
    try {
      HttpRequest request =
          HttpRequest.newBuilder(REGISTRATION_URL)
              .timeout(timeout)
              .header("Authorization", "ClientSecret " + clientSecret)
              .header("Content-Type", "application/json")
              .POST(HttpRequest.BodyPublishers.ofString(requestBody))
              .build();
      if (registrationClient == null) {
        registrationClient = withoutRedirects(client);
      }
      var pending = registrationClient.sendAsync(request, HttpResponse.BodyHandlers.discarding());
      try {
        response = pending.get(timeout.toNanos(), TimeUnit.NANOSECONDS);
      } catch (InterruptedException | TimeoutException e) {
        pending.cancel(true);
        throw e;
      }
    } catch (IllegalArgumentException e) {
      // Request-building errors can include the Authorization header value.
      throw new CdnException("invalid module profile registration request");
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new CdnException("module profile registration interrupted");
    } catch (TimeoutException | ExecutionException e) {
      throw new CdnException("module profile registration request failed");
    }
    int status = response.statusCode();
    if (status < 200 || status >= 300) {
      throw new CdnException("module profile registration: " + status);
    }
  }

  static HttpClient withoutRedirects(HttpClient client) {
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
