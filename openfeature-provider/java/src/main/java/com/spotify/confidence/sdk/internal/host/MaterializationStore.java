package com.spotify.confidence.sdk.internal.host;

import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import java.util.List;

/** The storage boundary for suspended resolves, as the Go host's host.MaterializationStore. */
public interface MaterializationStore {

  /** Answers the requested records. Omitted records are absent. */
  List<Record> read(List<Record> requested);

  /** Stores assignments the module produced. */
  default void write(List<Record> assignments) {}
}
