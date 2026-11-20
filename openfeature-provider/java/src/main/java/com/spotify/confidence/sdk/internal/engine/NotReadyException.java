package com.spotify.confidence.sdk.internal.engine;

/** No module has been installed yet. */
public final class NotReadyException extends RuntimeException {

  NotReadyException() {
    super("provider not ready: no resolver module loaded yet");
  }
}
