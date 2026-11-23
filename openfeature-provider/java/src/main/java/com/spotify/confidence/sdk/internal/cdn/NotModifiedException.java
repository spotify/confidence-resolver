package com.spotify.confidence.sdk.internal.cdn;

/** The module is unchanged since the last accepted ETag. */
public final class NotModifiedException extends CdnException {

  NotModifiedException() {
    super("module not modified");
  }
}
