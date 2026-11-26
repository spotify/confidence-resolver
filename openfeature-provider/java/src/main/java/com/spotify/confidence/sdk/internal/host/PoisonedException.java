package com.spotify.confidence.sdk.internal.host;

/** A call on a resolver whose module trapped (see {@link HostConfig#onPoison}). */
public final class PoisonedException extends RuntimeException {

  PoisonedException() {
    super("resolver module trapped; its state may be inconsistent");
  }
}
