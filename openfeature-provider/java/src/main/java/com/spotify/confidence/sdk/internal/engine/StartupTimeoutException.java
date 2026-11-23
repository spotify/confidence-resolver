package com.spotify.confidence.sdk.internal.engine;

/**
 * The first module didn't arrive within the startup budget. The engine keeps polling in the
 * background and fires {@code onReady} once a module installs; this is recoverable.
 */
public final class StartupTimeoutException extends RuntimeException {

  StartupTimeoutException(String message, Throwable cause) {
    super(message, cause);
  }
}
