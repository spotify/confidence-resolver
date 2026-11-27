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
  private final URI url;

  ProfileRegistration(String clientSecret, String profile, HttpClient client, String testUrl) {
    this.clientSecret = clientSecret;
    this.client = client;
    this.url = testUrl == null ? REGISTRATION_URL : URI.create(testUrl);
    this.requestBody = "{\"profile\":\"" + profile + "\"}";
  }

  void register(Duration remaining) {
    if (client.followRedirects() != HttpClient.Redirect.NEVER) {
      throw new CdnException(
          "module profile registration requires an HTTP client with redirects disabled");
    }
    Duration timeout = remaining.compareTo(REQUEST_TIMEOUT) < 0 ? remaining : REQUEST_TIMEOUT;
    final HttpResponse<Void> response;
    try {
      HttpRequest request =
          HttpRequest.newBuilder(url)
              .timeout(timeout)
              .header("Authorization", "ClientSecret " + clientSecret)
              .header("Content-Type", "application/json")
              .POST(HttpRequest.BodyPublishers.ofString(requestBody))
              .build();
      var pending = client.sendAsync(request, HttpResponse.BodyHandlers.discarding());
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
}
