package com.spotify.confidence.sdk.internal.cdn;

/** No module exists for this credential yet. Retryable: the module may be published later. */
public final class NotPublishedException extends CdnException {

  NotPublishedException() {
    super("no module published for this credential");
  }
}
