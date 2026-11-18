package com.spotify.confidence.sdk.internal.host;

import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import run.endive.wasm.Parser;
import run.endive.wasm.WasmModule;
import run.endive.wasm.types.Export;
import run.endive.wasm.types.FunctionImport;
import run.endive.wasm.types.FunctionType;
import run.endive.wasm.types.MemoryImport;
import run.endive.wasm.types.ValType;

/**
 * A compiled state-to-wasm resolver module, validated against the host contract: one shared {@code
 * host.memory} import, only the known function imports, the required exports with exact signatures,
 * and the optional exports (when present) likewise.
 *
 * <p>The embedding contract is state-to-wasm's MEMORY.md; the Go host in {@code
 * openfeature-provider/go/confidence/internal/host} is the reference.
 */
public final class Module {

  /** The parsed module. */
  public final WasmModule wasm;

  /** The shared memory's declared limits, in pages. */
  public final int minPages;

  public final int maxPages;

  /** The optional exports the module provides. */
  public final Set<String> optional;

  private static final Map<String, FunctionType> KNOWN_IMPORTS =
      Map.of(
          "env.write_logs", sig(List.of(ValType.I64), List.of()),
          "env.current_time", sig(List.of(), List.of(ValType.I64)),
          "env.random_seed", sig(List.of(), List.of(ValType.I64)),
          "apply_filter.seen", sig(List.of(ValType.I64), List.of(ValType.I32)),
          "schema_filter.seen", sig(List.of(ValType.I64), List.of(ValType.I32)));

  private static final Map<String, FunctionType> REQUIRED_EXPORTS =
      Map.of(
          "alloc", sig(List.of(ValType.I32), List.of(ValType.I32)),
          "free", sig(List.of(ValType.I32), List.of()),
          "resolve_process_start", sig(List.of(ValType.I64), List.of(ValType.I64)),
          "resolve_process_resume", sig(List.of(ValType.I32, ValType.I64), List.of(ValType.I64)),
          "resolve_process_discard", sig(List.of(ValType.I32), List.of()),
          "apply_flags", sig(List.of(ValType.I64), List.of(ValType.I32)),
          "flush_logs", sig(List.of(), List.of()),
          "ack_logs", sig(List.of(ValType.I64), List.of()),
          "destroy_instance", sig(List.of(), List.of()),
          "evaluate", sig(List.of(ValType.I64, ValType.I64), List.of(ValType.I64)));

  private static final Map<String, FunctionType> OPTIONAL_EXPORTS =
      Map.of(
          "allocated_bytes", sig(List.of(), List.of(ValType.I32)),
          "metrics_snapshot", sig(List.of(ValType.I32), List.of(ValType.I64)),
          "observe_client_resolve_latency", sig(List.of(ValType.I32, ValType.I64), List.of()),
          "unpack_bitsets", sig(List.of(), List.of()));

  private Module(WasmModule wasm, int minPages, int maxPages, Set<String> optional) {
    this.wasm = wasm;
    this.minPages = minPages;
    this.maxPages = maxPages;
    this.optional = optional;
  }

  /** Parses and validates the module; throws {@link InvalidModuleException} when it cannot run. */
  public static Module parse(byte[] bytes) {
    WasmModule module;
    try {
      module = Parser.parse(bytes);
    } catch (RuntimeException e) { // MalformedException included
      throw new InvalidModuleException("parse: " + e.getMessage());
    }
    return validate(module);
  }

  private static Module validate(WasmModule module) {
    MemoryImport memory = null;
    int memories = 0;
    Map<String, FunctionType> imports = new HashMap<>();
    int importedFunctions = 0;
    for (int i = 0; i < module.importSection().importCount(); i++) {
      var imp = module.importSection().getImport(i);
      if (imp instanceof MemoryImport mem) {
        memories++;
        memory = mem;
      } else if (imp instanceof FunctionImport fn) {
        importedFunctions++;
        imports.put(fn.module() + "." + fn.name(), module.typeSection().getType(fn.typeIndex()));
      } else {
        throw new InvalidModuleException("unknown import " + imp.module() + "." + imp.name());
      }
    }
    if (memories != 1) {
      throw new InvalidModuleException("expected one imported memory, got " + memories);
    }
    if (!memory.module().equals("host") || !memory.name().equals("memory")) {
      throw new InvalidModuleException(
          "memory must be imported as host.memory, got " + memory.module() + "." + memory.name());
    }
    // Endive maps an undeclared maximum to MAX_PAGES; the host caps well below either way.
    if (!memory.limits().shared()) {
      throw new InvalidModuleException("host.memory must be shared (threaded module variant)");
    }

    for (var e : KNOWN_IMPORTS.entrySet()) {
      FunctionType got = imports.get(e.getKey());
      if (got != null) {
        checkSig("import " + e.getKey(), got, e.getValue());
      }
    }
    for (String name : imports.keySet()) {
      if (!KNOWN_IMPORTS.containsKey(name)) {
        throw new InvalidModuleException("unknown import " + name);
      }
    }

    Map<String, FunctionType> exports = new HashMap<>();
    for (int i = 0; i < module.exportSection().exportCount(); i++) {
      Export e = module.exportSection().getExport(i);
      if (e.exportType() != run.endive.wasm.types.ExternalType.FUNCTION) {
        continue;
      }
      int typeIdx = module.functionSection().getFunctionType(e.index() - importedFunctions);
      exports.put(e.name(), module.typeSection().getType(typeIdx));
    }
    for (var e : REQUIRED_EXPORTS.entrySet()) {
      FunctionType got = exports.get(e.getKey());
      if (got == null) {
        throw new InvalidModuleException("missing export " + e.getKey());
      }
      checkSig("export " + e.getKey(), got, e.getValue());
    }
    Set<String> optional = new HashSet<>();
    for (var e : OPTIONAL_EXPORTS.entrySet()) {
      FunctionType got = exports.get(e.getKey());
      if (got == null) {
        continue;
      }
      checkSig("export " + e.getKey(), got, e.getValue());
      optional.add(e.getKey());
    }
    return new Module(
        module, memory.limits().initialPages(), memory.limits().maximumPages(), optional);
  }

  private static void checkSig(String what, FunctionType got, FunctionType want) {
    if (!got.params().equals(want.params()) || !got.returns().equals(want.returns())) {
      throw new InvalidModuleException(
          what
              + ": signature "
              + got.params()
              + " -> "
              + got.returns()
              + ", want "
              + want.params()
              + " -> "
              + want.returns());
    }
  }

  private static FunctionType sig(List<ValType> params, List<ValType> results) {
    return FunctionType.of(params, results);
  }
}
