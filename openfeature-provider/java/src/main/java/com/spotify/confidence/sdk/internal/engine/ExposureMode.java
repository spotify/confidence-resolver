package com.spotify.confidence.sdk.internal.engine;

/** How exposure (apply) events are deduplicated. */
public enum ExposureMode {
  /** The default: a persistent dedup filter collapses repeated applies. */
  DEDUPLICATED,
  /** No dedup: every apply is logged. */
  ALL,
  /** Suppress non-error exposure events. */
  NONE
}
