package com.spotify.confidence.sdk.internal.engine;

/** The engine has been closed. */
public final class EngineClosedException extends RuntimeException {

  EngineClosedException() {
    super("provider is closed");
  }
}
