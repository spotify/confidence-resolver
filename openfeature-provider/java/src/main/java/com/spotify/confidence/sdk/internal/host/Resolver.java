package com.spotify.confidence.sdk.internal.host;

import com.google.protobuf.InvalidProtocolBufferException;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.Materialization.Record;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import com.spotify.confidence.flags.wasm.v1.ResolveProcessResponse;
import com.spotify.confidence.flags.wasm.v1.ResolveProcessResponse.Error.Code;
import com.spotify.confidence.flags.wasm.v1.WriteLogsPayload;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Function;
import run.endive.compiler.MachineFactoryCompiler;
import run.endive.runtime.ByteArrayMemory;
import run.endive.runtime.ExportFunction;
import run.endive.runtime.HostFunction;
import run.endive.runtime.ImportMemory;
import run.endive.runtime.ImportValues;
import run.endive.runtime.Instance;
import run.endive.runtime.Machine;
import run.endive.runtime.Memory;
import run.endive.wasm.types.FunctionType;
import run.endive.wasm.types.MemoryLimits;
import run.endive.wasm.types.ValType;

/**
 * Hosts one module generation: its own shared memory and a lazily filled, bounded pool of
 * instances. Resolves with suspend/resume through a materialization store, evaluates for every flag
 * type, applies, flushes logs, and renders metrics.
 *
 * <p>Port of the Go host's {@code internal/host/resolver.go}; that file is the reference for
 * semantics, including the discard rules for abandoned suspended processes.
 */
public final class Resolver implements AutoCloseable {

  static final int PAGE_SIZE = 64 << 10;

  /** apply_flags' status for a client secret that matches no client. */
  private static final int APPLY_UNKNOWN_CLIENT = 4;

  private final Module module;
  private final HostConfig cfg;
  private final Function<Instance, Machine> machineFactory;
  private final ByteArrayMemory memory;
  private final ArrayBlockingQueue<ModuleInstance> idle;
  private final Object lock = new Object();
  private final List<ModuleInstance> all = new ArrayList<>();
  private int created;
  private final AtomicBoolean closed = new AtomicBoolean();
  private final SecureRandom random = new SecureRandom();

  public Resolver(Module module, HostConfig cfg) {
    this.module = module;
    this.cfg = cfg;
    this.machineFactory = cfg.interpreter ? null : MachineFactoryCompiler.compile(module.wasm);
    int maxPages = (int) Math.min(module.maxPages, cfg.maxMemoryBytes / PAGE_SIZE);
    if (maxPages < module.minPages) {
      throw new InvalidModuleException(
          "memory cap of "
              + cfg.maxMemoryBytes
              + " bytes is below the module's minimum of "
              + module.minPages
              + " pages");
    }
    this.memory = new ByteArrayMemory(new MemoryLimits(module.minPages, maxPages, true));
    this.idle = new ArrayBlockingQueue<>(Math.max(1, cfg.instances));
  }

  // ---------------------------------------------------------------- call surface

  /** Resolves req and evaluates each of evals against the result. */
  public List<ResolutionDetails> evaluate(
      ResolveFlagsRequest req, List<EvaluateRequest> evals, MaterializationStore store) {
    return process(req, evals, false, store).details;
  }

  /** Resolves req and returns the module's full response (for deferred apply). */
  public ResolveFlagsResponse resolve(ResolveFlagsRequest req, MaterializationStore store) {
    return process(req, null, true, store).response;
  }

  /** Forwards a serialized ApplyFlagsRequest. */
  public void applyFlags(byte[] request) {
    with(
        inst -> {
          long arg = inst.write(request);
          long status = inst.applyFn.apply(arg)[0];
          if (status == 0) {
            return null;
          }
          if (status == APPLY_UNKNOWN_CLIENT) {
            throw new ResolveRejectException(
                Code.UNAUTHENTICATED, "client secret matches no client");
          }
          throw new IllegalStateException("apply_flags failed with status " + status);
        });
  }

  /** Asks the module to emit buffered logs through the config's writeLogs callback. */
  public void flushLogs() {
    with(
        inst -> {
          inst.flush.apply();
          return null;
        });
  }

  /** Loads every segment bitset up front, if the module supports it. */
  public void warm() {
    if (!module.optional.contains("unpack_bitsets")) {
      return;
    }
    with(
        inst -> {
          inst.unpackBitsets.apply();
          return null;
        });
  }

  /** Renders the module's cumulative telemetry as Prometheus text, or OpenMetrics. */
  public String metrics(boolean openMetrics) {
    if (!module.optional.contains("metrics_snapshot")) {
      return "";
    }
    return with(
        inst -> {
          long format = openMetrics ? 1 : 0;
          long slice = call1(inst.metrics, format);
          try {
            return new String(inst.readSlice(slice), StandardCharsets.UTF_8);
          } finally {
            inst.freeSlice(slice);
          }
        });
  }

  /** The module's self-reported allocated bytes; requires the allocated_bytes export. */
  public long allocatedBytes() {
    if (!module.optional.contains("allocated_bytes")) {
      throw new IllegalStateException("module does not export allocated_bytes");
    }
    return with(inst -> inst.allocatedBytes.apply()[0]);
  }

  /** The number of resolve_process_discard calls served so far. */
  public int discardCalls() {
    synchronized (lock) {
      return all.stream().mapToInt(i -> i.discards).sum();
    }
  }

  /**
   * Flushes logs on the first instance, retires every instance, and releases the engine. The caller
   * must ensure no calls are in flight; later calls fail with {@link ClosedException}.
   */
  public void close() {
    if (!closed.compareAndSet(false, true)) {
      return;
    }
    List<Throwable> errors = new ArrayList<>();
    List<ModuleInstance> snapshot;
    synchronized (lock) {
      snapshot = new ArrayList<>(all);
    }
    for (int i = 0; i < snapshot.size(); i++) {
      ModuleInstance inst = snapshot.get(i);
      if (i == 0) {
        try {
          inst.flush.apply();
        } catch (RuntimeException e) {
          errors.add(e);
        }
      }
      try {
        inst.destroy.apply();
      } catch (RuntimeException e) {
        errors.add(e);
      }
      try {
        inst.instance.close();
      } catch (RuntimeException e) {
        errors.add(e);
      }
      for (Instance filter : inst.filterInstances) {
        try {
          filter.close();
        } catch (RuntimeException e) {
          errors.add(e);
        }
      }
    }
    if (!errors.isEmpty()) {
      RuntimeException e = new RuntimeException("close failed (" + errors.size() + " errors)");
      errors.forEach(e::addSuppressed);
      throw e;
    }
  }

  // ---------------------------------------------------------------- the resolve process

  /** What one start or resume call produced. */
  private static final class Outcome {

    List<ResolutionDetails> details = List.of();
    ResolveFlagsResponse response;
    List<Record> writes = List.of();
    Suspension suspended;
  }

  private record Suspension(int processId, List<Record> reads) {}

  private Outcome process(
      ResolveFlagsRequest req,
      List<EvaluateRequest> evals,
      boolean wantResponse,
      MaterializationStore store) {
    long started = System.nanoTime();
    byte[] requestBytes = req.toByteArray();
    Outcome out =
        with(
            inst -> {
              long arg = inst.write(requestBytes);
              long env = call1(inst.start, arg);
              try {
                return finish(inst, env, evals, wantResponse, started);
              } finally {
                inst.freeSlice(env);
              }
            });
    if (out.suspended == null) {
      writeToStore(store, out.writes);
      return out;
    }

    // From here this call owns the suspended process: every path that doesn't resume it
    // discards it, or its context stays allocated in the module's memory.
    int id = out.suspended.processId;
    byte[] materialization;
    try {
      materialization = records(store.read(out.suspended.reads));
    } catch (RuntimeException e) {
      RuntimeException read = new RuntimeException("materialization read: " + e.getMessage(), e);
      discard(id).forEach(read::addSuppressed);
      throw read;
    }
    boolean[] resumed = {false};
    try {
      out =
          with(
              inst -> {
                long arg = inst.write(materialization); // not resumed on failure: discarded below
                resumed[0] = true;
                long env = call1(inst.resumeFn, id, arg);
                try {
                  Outcome o = finish(inst, env, evals, wantResponse, started);
                  if (o.suspended != null) {
                    inst.discardProcess(o.suspended.processId);
                    throw new IllegalStateException("resolve suspended after resume");
                  }
                  return o;
                } finally {
                  inst.freeSlice(env);
                }
              });
    } catch (RuntimeException e) {
      if (!resumed[0]) {
        discard(id).forEach(e::addSuppressed);
      }
      throw e;
    }
    writeToStore(store, out.writes);
    return out;
  }

  /** Decodes a process envelope and evaluates against it. The caller frees the envelope. */
  private Outcome finish(
      ModuleInstance inst,
      long envSlice,
      List<EvaluateRequest> evals,
      boolean wantResponse,
      long started) {
    ResolveProcessResponse env = parseProcess(inst.readSlice(envSlice));
    switch (env.getResultCase()) {
      case ERROR ->
          throw new ResolveRejectException(env.getError().getCode(), env.getError().getMessage());
      case SUSPENDED -> {
        Suspension s =
            new Suspension(
                env.getSuspended().getProcessId(),
                env.getSuspended().getMaterializationToReadList());
        Outcome out = new Outcome();
        out.suspended = s;
        return out;
      }
      case RESOLVED -> {
        if (inst.latency != null) {
          long micros = (System.nanoTime() - started) / 1_000;
          inst.latency.apply(env.getResolved().getClientIndex(), micros);
        }
        Outcome out = new Outcome();
        if (wantResponse) {
          out.response = env.getResolved().getResponse();
        }
        out.writes = env.getResolved().getMaterializationToWriteList();
        if (evals != null && !evals.isEmpty()) {
          List<ResolutionDetails> details = new ArrayList<>(evals.size());
          for (EvaluateRequest eval : evals) {
            long arg = inst.write(eval.toByteArray());
            long outSlice = call1(inst.evaluate, envSlice, arg);
            try {
              details.add(parseDetails(inst.readSlice(outSlice)));
            } finally {
              inst.freeSlice(outSlice);
            }
          }
          out.details = details;
        }
        return out;
      }
      default -> throw new IllegalStateException("empty process response");
    }
  }

  private void writeToStore(MaterializationStore store, List<Record> writes) {
    if (writes.isEmpty()) {
      return;
    }
    try {
      store.write(writes);
    } catch (RuntimeException e) {
      throw new IllegalStateException("materialization write: " + e.getMessage(), e);
    }
  }

  /** Discards a suspended process on any instance, even one that must be waited for. */
  private List<Throwable> discard(int id) {
    try {
      with(
          inst -> {
            inst.discardProcess(id);
            return null;
          });
      return List.of();
    } catch (ClosedException e) {
      return List.of(); // a closed resolver has freed the process
    } catch (RuntimeException e) {
      return List.of(e);
    }
  }

  private static byte[] records(List<Record> records) {
    return com.spotify.confidence.flags.wasm.v1.Materialization.newBuilder()
        .addAllRecords(records)
        .build()
        .toByteArray();
  }

  // ---------------------------------------------------------------- instances

  private interface ModuleCall<T> {

    T run(ModuleInstance inst);
  }

  private <T> T with(ModuleCall<T> call) {
    if (closed.get()) {
      throw new ClosedException();
    }
    ModuleInstance inst = claim();
    try {
      return call.run(inst);
    } finally {
      release(inst);
    }
  }

  /** Returns an idle instance, creating one while under the cap. Blocks while all are busy. */
  private ModuleInstance claim() {
    ModuleInstance inst = idle.poll();
    if (inst != null) {
      return inst;
    }
    synchronized (lock) {
      if (closed.get()) {
        throw new ClosedException();
      }
      if (created < cfg.instances) {
        created++; // reserve the slot; released on failure
        try {
          inst = newInstance();
        } catch (RuntimeException e) {
          created--;
          throw new InvalidModuleException("instantiate: " + e.getMessage());
        }
        all.add(inst);
        return inst;
      }
    }
    try {
      return idle.take();
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new ClosedException();
    }
  }

  private void release(ModuleInstance inst) {
    idle.offer(inst);
  }

  private ModuleInstance newInstance() {
    ImportValues.Builder imports = ImportValues.builder();
    imports.addMemory(new ImportMemory("host", "memory", memory));
    imports.addFunction(
        new HostFunction(
            "env",
            "write_logs",
            FunctionType.of(List.of(ValType.I64), List.of()),
            (instance, args) -> {
              writeLogs(instance, args[0]);
              return null;
            }));
    imports.addFunction(
        new HostFunction(
            "env",
            "current_time",
            FunctionType.of(List.of(), List.of(ValType.I64)),
            (instance, args) -> new long[] {System.currentTimeMillis()}));
    imports.addFunction(
        new HostFunction(
            "env",
            "random_seed",
            FunctionType.of(List.of(), List.of(ValType.I64)),
            (instance, args) -> new long[] {randomSeed()}));
    List<Instance> filterInstances = new ArrayList<>(2);
    Instance apply = linkFilter(imports, "apply_filter", cfg.applyFilter);
    Instance schema = linkFilter(imports, "schema_filter", cfg.schemaFilter);
    if (apply != null) {
      filterInstances.add(apply);
    }
    if (schema != null) {
      filterInstances.add(schema);
    }
    Instance.Builder builder = Instance.builder(module.wasm).withImportValues(imports.build());
    if (machineFactory != null) {
      builder.withMachineFactory(machineFactory);
    }
    return new ModuleInstance(builder.build(), filterInstances, module.optional);
  }

  private Instance linkFilter(ImportValues.Builder imports, String name, Filter filter) {
    Filter.Link link = filter.newLink(name);
    imports.addFunction(link.function());
    return link.instance(); // null for filter constants; closed with the instance
  }

  /** The host's write_logs import: copy the payload's parts out, hand them off, then ack. */
  void writeLogs(Instance instance, long payload) {
    boolean acked = false;
    try {
      Memory mem = instance.memory();
      WriteLogsPayload env = parsePayload(readSlice(mem, payload));
      int size = 0;
      for (long part : env.getPartsList()) {
        size += (int) (part >>> 32);
      }
      byte[] buf = new byte[size];
      int offset = 0;
      for (long part : env.getPartsList()) {
        byte[] bytes = readSlice(mem, part);
        System.arraycopy(bytes, 0, buf, offset, bytes.length);
        offset += bytes.length;
      }
      cfg.writeLogs.accept(buf);
      ack(instance, payload); // re-entrant: the module's export is on the stack
      acked = true;
    } catch (Throwable t) {
      cfg.reportFailure.accept(t);
    } finally {
      if (!acked) {
        ack(instance, payload);
      }
    }
  }

  private void ack(Instance instance, long payload) {
    try {
      instance.export("ack_logs").apply(payload);
    } catch (RuntimeException e) {
      cfg.reportFailure.accept(e);
    }
  }

  private long randomSeed() {
    byte[] bytes = new byte[8];
    random.nextBytes(bytes);
    long value = 0;
    for (int i = 7; i >= 0; i--) {
      value = (value << 8) | (bytes[i] & 0xff);
    }
    return value;
  }

  private static byte[] readSlice(Memory memory, long slice) {
    return memory.readBytes((int) slice, (int) (slice >>> 32));
  }

  private static long call1(ExportFunction fn, long... args) {
    long result = fn.apply(args)[0];
    if (result == 0) {
      throw new IllegalStateException("module returned a null slice");
    }
    return result;
  }

  private static ResolveProcessResponse parseProcess(byte[] bytes) {
    try {
      return ResolveProcessResponse.parseFrom(bytes);
    } catch (InvalidProtocolBufferException e) {
      throw new IllegalStateException("malformed process response", e);
    }
  }

  private static ResolutionDetails parseDetails(byte[] bytes) {
    try {
      return ResolutionDetails.parseFrom(bytes);
    } catch (InvalidProtocolBufferException e) {
      throw new IllegalStateException("malformed resolution details", e);
    }
  }

  private static WriteLogsPayload parsePayload(byte[] bytes) {
    try {
      return WriteLogsPayload.parseFrom(bytes);
    } catch (InvalidProtocolBufferException e) {
      throw new IllegalStateException("malformed write_logs payload", e);
    }
  }

  // ---------------------------------------------------------------- one instance

  /** One module instance with its exports. Instances are not thread-safe; the pool serializes. */
  private static final class ModuleInstance {

    final Instance instance;
    final List<Instance> filterInstances;
    final ExportFunction alloc;
    final ExportFunction freeFn;
    final ExportFunction start;
    final ExportFunction resumeFn;
    final ExportFunction discardFn;
    final ExportFunction applyFn;
    final ExportFunction flush;
    final ExportFunction evaluate;
    final ExportFunction destroy;
    final ExportFunction metrics; // null when not exported
    final ExportFunction allocatedBytes;
    final ExportFunction unpackBitsets;
    final ExportFunction latency;
    int discards;

    ModuleInstance(Instance instance, List<Instance> filterInstances, Set<String> optional) {
      this.instance = instance;
      this.filterInstances = filterInstances;
      this.alloc = instance.export("alloc");
      this.freeFn = instance.export("free");
      this.start = instance.export("resolve_process_start");
      this.resumeFn = instance.export("resolve_process_resume");
      this.discardFn = instance.export("resolve_process_discard");
      this.applyFn = instance.export("apply_flags");
      this.flush = instance.export("flush_logs");
      this.evaluate = instance.export("evaluate");
      this.destroy = instance.export("destroy_instance");
      this.metrics = optionalExport(instance, optional, "metrics_snapshot");
      this.allocatedBytes = optionalExport(instance, optional, "allocated_bytes");
      this.unpackBitsets = optionalExport(instance, optional, "unpack_bitsets");
      this.latency = optionalExport(instance, optional, "observe_client_resolve_latency");
    }

    private static ExportFunction optionalExport(
        Instance instance, Set<String> optional, String name) {
      return optional.contains(name) ? instance.export(name) : null;
    }

    /** Copies bytes into a fresh module allocation and returns them as a slice argument. */
    long write(byte[] bytes) {
      long pointer = alloc.apply(bytes.length)[0];
      if (pointer == 0 && bytes.length > 0) {
        throw new AllocationException(bytes.length + " bytes");
      }
      instance.memory().write((int) pointer, bytes);
      return (pointer & 0xffffffffL) | ((long) bytes.length << 32);
    }

    byte[] readSlice(long slice) {
      return instance.memory().readBytes((int) slice, (int) (slice >>> 32));
    }

    void freeSlice(long slice) {
      freeFn.apply(slice & 0xffffffffL);
    }

    void discardProcess(int id) {
      discards++;
      discardFn.apply(id);
    }
  }
}
