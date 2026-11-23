package com.spotify.confidence.sdk;

import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.sdk.MaterializationStore.ReadOp;
import com.spotify.confidence.sdk.MaterializationStore.ReadResult;
import com.spotify.confidence.sdk.MaterializationStore.WriteOp;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.Executor;
import org.slf4j.Logger;

/**
 * Adapts the public {@link MaterializationStore} to the module's flat records: a record with a rule
 * is a variant (sticky assignment) read, one without is an inclusion read. Absent results mean "no
 * assignment" and "not included".
 *
 * <p>Reads join the store's {@link CompletionStage}; the store therefore holds no evaluation thread
 * of ours while it works. Writes are fire-and-forget, as in earlier versions: a slow or failing
 * store never fails or delays a resolve. {@link MaterializationNotSupportedException} on writes is
 * swallowed, since the default store raises it.
 */
final class StoreAdapter implements com.spotify.confidence.sdk.internal.host.MaterializationStore {

  private final MaterializationStore store;
  private final Logger logger;
  private final Executor writes;

  StoreAdapter(MaterializationStore store, Logger logger, Executor writes) {
    this.store = store;
    this.logger = logger;
    this.writes = writes;
  }

  @Override
  public List<Record> read(List<Record> requested) {
    if (requested.isEmpty()) {
      return List.of();
    }
    List<ReadOp> ops = new ArrayList<>(requested.size());
    for (Record r : requested) {
      if (r == null) {
        continue;
      }
      if (!r.getRule().isEmpty()) {
        ops.add(new ReadOp.Variant(r.getMaterialization(), r.getUnit(), r.getRule()));
      } else {
        ops.add(new ReadOp.Inclusion(r.getMaterialization(), r.getUnit()));
      }
    }
    List<ReadResult> results;
    try {
      results = store.read(ops).toCompletableFuture().join();
    } catch (CompletionException e) {
      Throwable cause = e.getCause() == null ? e : e.getCause();
      if (cause instanceof CompletionException cc && cc.getCause() != null) {
        cause = cc.getCause();
      }
      throw cause instanceof RuntimeException re ? re : new RuntimeException(cause);
    } catch (MaterializationNotSupportedException e) {
      throw new RuntimeException(e);
    }
    List<Record> records = new ArrayList<>(results.size());
    for (ReadResult result : results) {
      if (result instanceof ReadResult.Variant v) {
        String variant = v.variant().orElse(null);
        if (variant != null && !variant.isEmpty()) {
          records.add(
              Record.newBuilder()
                  .setUnit(v.unit())
                  .setMaterialization(v.materialization())
                  .setRule(v.rule())
                  .setVariant(variant)
                  .build());
        }
      } else if (result instanceof ReadResult.Inclusion i && i.included()) {
        records.add(
            Record.newBuilder().setUnit(i.unit()).setMaterialization(i.materialization()).build());
      }
    }
    return records;
  }

  @Override
  public void write(List<Record> assignments) {
    if (assignments.isEmpty()) {
      return;
    }
    Set<WriteOp> ops = new LinkedHashSet<>(assignments.size() * 2);
    for (Record r : assignments) {
      if (r != null) {
        ops.add(
            new WriteOp.Variant(r.getMaterialization(), r.getUnit(), r.getRule(), r.getVariant()));
      }
    }
    CompletableFuture.runAsync(
        () -> {
          try {
            store.write(ops).toCompletableFuture().join();
          } catch (CompletionException e) {
            report(e.getCause() == null ? e : e.getCause());
          } catch (MaterializationNotSupportedException e) {
            // The default store; the module already resolved without materializations.
          }
        },
        writes);
  }

  private void report(Throwable t) {
    if (t instanceof CompletionException cc && cc.getCause() != null) {
      t = cc.getCause();
    }
    if (t instanceof MaterializationNotSupportedException) {
      return;
    }
    logger.warn("materialization write failed", t);
  }
}
