package com.spotify.confidence.sdk.internal.engine;

/** A startup failure that retrying won't fix: a rejected credential or an invalid first module. */
public final class TerminalStartupException extends RuntimeException {

  TerminalStartupException(String message, Throwable cause) {
    super(message, cause);
  }
}
