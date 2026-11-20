package com.spotify.confidence.sdk.internal.host;

/**
 * The calling thread was already interrupted when it asked for a module call. Nothing entered the
 * module, so this is an ordinary error: the caller's interrupt status is preserved (unchecked, as
 * the JVM's unchecked-interrupt convention) for the surrounding code to notice.
 */
public class InterruptedCallException extends RuntimeException {

  public InterruptedCallException(String message) {
    super(message);
  }
}
