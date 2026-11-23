package com.spotify.confidence.sdk.internal.cdn;

/**
 * A failure retrying won't fix, such as a rejected credential or an artifact that doesn't decrypt.
 * The engine treats this as fatal while no module is installed.
 */
public final class TerminalCdnException extends CdnException {

  TerminalCdnException(String message) {
    super(message);
  }

  TerminalCdnException(String message, Throwable cause) {
    super(message, cause);
  }
}
