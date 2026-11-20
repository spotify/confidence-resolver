package com.spotify.confidence.sdk;

/**
 * Configuration for {@link OpenFeatureLocalResolveProvider#getPrometheusMetrics}.
 *
 * <p><b>Experimental:</b> this API is subject to change.
 */
public class SnapshotConfig {

  private final boolean openMetrics;

  /** Creates a new config with Prometheus-format output. */
  public SnapshotConfig() {
    this(false);
  }

  /** Creates a new config selecting OpenMetrics format (with {@code # HELP/# TYPE} comments). */
  public SnapshotConfig(boolean openMetrics) {
    this.openMetrics = openMetrics;
  }

  /** True for OpenMetrics output, false for classic Prometheus text format. */
  public boolean isOpenMetrics() {
    return openMetrics;
  }
}
