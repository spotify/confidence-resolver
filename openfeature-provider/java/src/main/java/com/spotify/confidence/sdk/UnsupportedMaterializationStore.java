package com.spotify.confidence.sdk;

import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletionStage;

/**
 * A materialization store that doesn't support materializations.
 *
 * <p>This is the default materialization store. When a flag requires materialization data (sticky
 * assignments or custom targeting), the evaluation will fail with {@link
 * MaterializationNotSupportedException}.
 *
 * <p>To enable materialization support, use either:
 *
 * <ul>
 *   <li>{@link RemoteMaterializationStore} - stores data remotely via gRPC to Confidence
 *   <li>Custom implementation of {@link MaterializationStore} - stores data in your own
 *       infrastructure
 * </ul>
 */
final class UnsupportedMaterializationStore implements MaterializationStore {

  @Override
  public CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops) {
    throw new MaterializationNotSupportedException();
  }

  @Override
  public CompletionStage<Void> write(Set<? extends WriteOp> ops) {
    throw new MaterializationNotSupportedException();
  }
}
