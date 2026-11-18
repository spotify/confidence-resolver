package com.spotify.confidence.sdk.internal.host;

import java.util.List;
import run.endive.runtime.HostFunction;
import run.endive.runtime.ImportFunction;
import run.endive.runtime.Instance;
import run.endive.wasm.types.FunctionType;
import run.endive.wasm.types.ValType;

/**
 * Answers one of a module's dedup imports ({@code apply_filter.seen}, {@code schema_filter.seen}).
 * Each resolver instance gets its own link; a {@link DedupeFilter}'s memory outlives resolver
 * generations, so dedup history survives a module swap.
 */
public interface Filter {

  /** One link: the import to wire into a main-module instance, and the instance behind it. */
  record Link(ImportFunction function, Instance instance) {}

  /** Supplies the {@code seen} import under the given module name for one main-module instance. */
  Link newLink(String importModule);

  /** Reports every key as new: no dedup. */
  static Filter neverSeen() {
    return constant(0);
  }

  /** Reports every key as seen, suppressing everything the filter gates. */
  static Filter alwaysSeen() {
    return constant(1);
  }

  private static Filter constant(int answer) {
    return importModule ->
        new Link(
            new HostFunction(
                importModule,
                "seen",
                FunctionType.of(List.of(ValType.I64), List.of(ValType.I32)),
                (instance, args) -> new long[] {answer}),
            null);
  }
}
