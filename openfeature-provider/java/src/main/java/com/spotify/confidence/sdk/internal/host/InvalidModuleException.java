package com.spotify.confidence.sdk.internal.host;

/** A module this host cannot run. The message says why. */
public final class InvalidModuleException extends RuntimeException {

  InvalidModuleException(String detail) {
    super("invalid resolver module: " + detail);
  }
}
