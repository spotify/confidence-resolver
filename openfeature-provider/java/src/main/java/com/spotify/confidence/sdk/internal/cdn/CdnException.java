package com.spotify.confidence.sdk.internal.cdn;

/**
 * A retryable module-CDN failure: the server or network is misbehaving (408, 429, 5xx, network
 * errors, 404s while the credential has no module yet are signaled by their subclasses).
 */
public class CdnException extends RuntimeException {

  CdnException(String message) {
    super(message);
  }

  CdnException(String message, Throwable cause) {
    super(message, cause);
  }
}
