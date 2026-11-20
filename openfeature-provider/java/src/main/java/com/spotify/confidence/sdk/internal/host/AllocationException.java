package com.spotify.confidence.sdk.internal.host;

/** A module allocation failed: its memory is exhausted. Never silently "no materialization". */
public final class AllocationException extends RuntimeException {

  AllocationException(String detail) {
    super("module allocation failed: " + detail);
  }
}
