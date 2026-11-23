package com.spotify.confidence.sdk.internal.host;

import java.io.IOException;
import java.io.InputStream;
import run.endive.runtime.ByteArrayMemory;
import run.endive.runtime.ImportFunction;
import run.endive.runtime.ImportMemory;
import run.endive.runtime.ImportValues;
import run.endive.runtime.Instance;
import run.endive.wasm.Parser;
import run.endive.wasm.WasmModule;
import run.endive.wasm.types.MemoryLimits;

/**
 * A fixed-size dedup filter: a shared memory of {@code pages} 64 KiB pages, written through
 * instances of state-to-wasm's threaded dedupe filter module (embedded as a resource, same build as
 * the shared testdata modules). The memory outlives the resolvers that link to it, so dedup history
 * survives module swaps.
 */
public final class DedupeFilter implements Filter {

  private static final String RESOURCE = "/wasm/dedupe-filter-threaded.wasm";

  private static volatile WasmModule filterModule;

  private final ByteArrayMemory memory;

  public DedupeFilter(int pages) {
    this.memory = new ByteArrayMemory(new MemoryLimits(pages, pages, true));
  }

  /** The memory backing this filter; shared by every instance this filter links. */
  public ByteArrayMemory memory() {
    return memory;
  }

  @Override
  public Link newLink(String importModule) {
    WasmModule wasm = filterModule();
    Instance instance =
        Instance.builder(wasm)
            .withImportValues(
                ImportValues.builder().addMemory(new ImportMemory("env", "memory", memory)).build())
            .build();
    return new Link(
        ImportFunction.exportAsImport(importModule, "seen", instance, "seen"), instance);
  }

  private static WasmModule filterModule() {
    WasmModule module = filterModule;
    if (module == null) {
      synchronized (DedupeFilter.class) {
        module = filterModule;
        if (module == null) {
          try (InputStream in = DedupeFilter.class.getResourceAsStream(RESOURCE)) {
            if (in == null) {
              throw new IllegalStateException("missing resource " + RESOURCE);
            }
            module = Parser.parse(in.readAllBytes());
          } catch (IOException e) {
            throw new IllegalStateException("unreadable resource " + RESOURCE, e);
          }
          filterModule = module;
        }
      }
    }
    return module;
  }
}
