package com.spotify.confidence.sdk.internal.host;

/** A call on a closed resolver. */
public final class ClosedException extends RuntimeException {

  ClosedException() {
    super("resolver is closed");
  }
}
