package com.spotify.confidence.sdk;

import com.dylibso.chicory.runtime.ExportFunction;
import com.dylibso.chicory.runtime.ImportFunction;
import com.dylibso.chicory.runtime.ImportValues;
import com.dylibso.chicory.runtime.Instance;
import com.dylibso.chicory.runtime.Memory;
import com.dylibso.chicory.wasm.types.FunctionType;
import com.dylibso.chicory.wasm.types.ValType;
import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import com.google.protobuf.Timestamp;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.LogMessage;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import com.spotify.confidence.sdk.wasm.Messages;
import java.time.Instant;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicInteger;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import java.util.concurrent.locks.ReentrantLock;
import java.util.function.Consumer;
import java.util.function.Function;

/**
 * Lowest layer of the compositional resolver: direct WASM interaction. Each instance wraps a single
 * Chicory WASM instance and is NOT thread-safe without external synchronization — the internal
 * {@link ReentrantLock} provides mutual exclusion for all operations.
 */
class WasmLocalResolver implements LocalResolver {
  private static final Logger logger = LoggerFactory.getLogger(WasmLocalResolver.class);
  private static final AtomicInteger INSTANCE_COUNTER = new AtomicInteger(0);
  private final FunctionType HOST_FN_TYPE =
      FunctionType.of(List.of(ValType.I32), List.of(ValType.I32));
  private final Instance instance;
  private final String instanceId;
  private boolean closed = false;

  // interop
  private final ExportFunction wasmMsgAlloc;
  private final ExportFunction wasmMsgFree;
  private final Consumer<WriteFlagLogsRequest> logSink;

  // api
  private final ExportFunction wasmMsgGuestSetResolverState;
  private final ExportFunction wasmMsgBoundedFlushLogs;
  private final ExportFunction wasmMsgBoundedFlushAssign;
  private final ExportFunction wasmMsgGuestApplyFlags;
  private final ExportFunction wasmMsgGuestResolveProcess;
  private final ExportFunction wasmMsgGuestPrometheusSnapshot;
  private final ReentrantLock lock = new ReentrantLock();

  public WasmLocalResolver(Consumer<WriteFlagLogsRequest> logSink) {
    this.logSink = logSink;
    this.instanceId = String.valueOf(INSTANCE_COUNTER.getAndIncrement());
    instance =
        Instance.builder(ConfidenceResolverModule.load())
            .withImportValues(
                ImportValues.builder()
                    .addFunction(
                        createImportFunction(
                            "current_time", Messages.Void::parseFrom, this::currentTime))
                    .addFunction(
                        createImportFunction("log_message", LogMessage::parseFrom, this::log))
                    .addFunction(
                        new ImportFunction(
                            "wasm_msg",
                            "wasm_msg_current_thread_id",
                            FunctionType.of(List.of(), List.of(ValType.I32)),
                            this::currentThreadId))
                    .build())
            .withMachineFactory(ConfidenceResolverModule::create)
            .build();
    wasmMsgAlloc = instance.export("wasm_msg_alloc");
    wasmMsgFree = instance.export("wasm_msg_free");
    wasmMsgGuestSetResolverState = instance.export("wasm_msg_guest_set_resolver_state");
    wasmMsgBoundedFlushLogs = instance.export("wasm_msg_guest_bounded_flush_logs");
    wasmMsgBoundedFlushAssign = instance.export("wasm_msg_guest_bounded_flush_assign");
    wasmMsgGuestApplyFlags = instance.export("wasm_msg_guest_apply_flags");
    wasmMsgGuestResolveProcess = instance.export("wasm_msg_guest_resolve_flags");
    wasmMsgGuestPrometheusSnapshot = instance.export("wasm_msg_guest_prometheus_snapshot");
  }

  private Message log(LogMessage message) {
    System.out.println(message.getMessage());
    return Messages.Void.getDefaultInstance();
  }

  private long[] currentThreadId(Instance instance, long... longs) {
    return new long[] {0};
  }

  private Timestamp currentTime(Messages.Void unused) {
    final Instant now = Instant.now();
    return Timestamp.newBuilder().setSeconds(now.getEpochSecond()).setNanos(now.getNano()).build();
  }

  @Override
  public void setResolverState(byte[] state, String accountId) {
    lock.lock();
    try {
      final var resolverStateRequest =
          Messages.SetResolverStateRequest.newBuilder()
              .setState(ByteString.copyFrom(state))
              .setAccountId(accountId)
              .build();
      final byte[] request =
          Messages.Request.newBuilder()
              .setData(ByteString.copyFrom(resolverStateRequest.toByteArray()))
              .build()
              .toByteArray();
      final int addr = transfer(request);
      final int respPtr = (int) wasmMsgGuestSetResolverState.apply(addr)[0];
      consumeResponse(respPtr, Messages.Void::parseFrom);
    } finally {
      lock.unlock();
    }
  }

  @Override
  public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
    lock.lock();
    try {
      if (closed) {
        throw new RuntimeException("Resolver is closed");
      }
      final int reqPtr = transferRequest(request);
      final int respPtr = (int) wasmMsgGuestResolveProcess.apply(reqPtr)[0];
      return CompletableFuture.completedFuture(
          consumeResponse(respPtr, ResolveProcessResponse::parseFrom));
    } finally {
      lock.unlock();
    }
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    lock.lock();
    try {
      if (closed) {
        throw new RuntimeException("Resolver is closed");
      }
      final int reqPtr = transferRequest(request);
      final int respPtr = (int) wasmMsgGuestApplyFlags.apply(reqPtr)[0];
      consumeResponse(respPtr, ApplyFlagsResponse::parseFrom);
    } finally {
      lock.unlock();
    }
  }

  @Override
  public void flushAllLogs() {
    lock.lock();
    try {
      if (closed) {
        return;
      }
      final var voidRequest = Messages.Void.getDefaultInstance();
      final var reqPtr = transferRequest(voidRequest);
      final var respPtr = (int) wasmMsgBoundedFlushLogs.apply(reqPtr)[0];
      final var request = consumeResponse(respPtr, WriteFlagLogsRequest::parseFrom);
      if (!isEmptyLogRequest(request)) {
        logSink.accept(request);
      }
    } finally {
      lock.unlock();
    }
  }

  @Override
  public void flushAssignLogs() {
    lock.lock();
    try {
      if (closed) {
        return;
      }
      final var voidRequest = Messages.Void.getDefaultInstance();
      final var reqPtr = transferRequest(voidRequest);
      final var respPtr = (int) wasmMsgBoundedFlushAssign.apply(reqPtr)[0];
      final var request = consumeResponse(respPtr, WriteFlagLogsRequest::parseFrom);
      if (!isEmptyLogRequest(request)) {
        logSink.accept(request);
      }
    } finally {
      lock.unlock();
    }
  }

  @Override
  public void close() {
    lock.lock();
    try {
      final var voidRequest = Messages.Void.getDefaultInstance();

      // Drain all pending assign logs (bounded flush may require multiple calls)
      while (true) {
        final var assignReqPtr = transferRequest(voidRequest);
        final var assignRespPtr = (int) wasmMsgBoundedFlushAssign.apply(assignReqPtr)[0];
        final var assignRequest = consumeResponse(assignRespPtr, WriteFlagLogsRequest::parseFrom);
        if (assignRequest.getFlagAssignedCount() == 0) {
          break;
        }
        logSink.accept(assignRequest);
      }

      // Final flush of resolve logs (also drains any remaining assigns)
      final var reqPtr = transferRequest(voidRequest);
      final var respPtr = (int) wasmMsgBoundedFlushLogs.apply(reqPtr)[0];
      final var request = consumeResponse(respPtr, WriteFlagLogsRequest::parseFrom);
      if (!isEmptyLogRequest(request)) {
        logSink.accept(request);
      }
      closed = true;
    } finally {
      lock.unlock();
    }
  }

  @Override
  public String prometheusSnapshot() {
    lock.lock();
    try {
      final var snapshotRequest =
          Messages.PrometheusSnapshotRequest.newBuilder().setInstance(instanceId).build();
      final int reqPtr = transferRequest(snapshotRequest);
      final int respPtr = (int) wasmMsgGuestPrometheusSnapshot.apply(reqPtr)[0];
      final var response = consumeResponse(respPtr, Messages.PrometheusSnapshotResponse::parseFrom);
      return response.getText();
    } catch (RuntimeException e) {
      logger.warn("prometheus snapshot failed", e);
      return "";
    } finally {
      lock.unlock();
    }
  }

  private static boolean isEmptyLogRequest(WriteFlagLogsRequest request) {
    return request.getFlagAssignedCount() == 0
        && request.getClientResolveInfoCount() == 0
        && request.getFlagResolveInfoCount() == 0;
  }

  private <T extends Message> T consumeResponse(int addr, ParserFn<T> codec) {
    try {
      final Messages.Response response = Messages.Response.parseFrom(consume(addr));
      if (response.hasError()) {
        throw new RuntimeException(response.getError());
      } else {
        return codec.apply(response.getData().toByteArray());
      }
    } catch (InvalidProtocolBufferException e) {
      throw new RuntimeException(e);
    }
  }

  private <T extends Message> T consumeRequest(int addr, ParserFn<T> codec) {
    try {
      final Messages.Request request = Messages.Request.parseFrom(consume(addr));
      return codec.apply(request.getData().toByteArray());
    } catch (InvalidProtocolBufferException e) {
      throw new RuntimeException(e);
    }
  }

  private int transferRequest(Message message) {
    final byte[] request =
        Messages.Request.newBuilder().setData(message.toByteString()).build().toByteArray();
    return transfer(request);
  }

  private int transferResponseSuccess(Message response) {
    final byte[] wrapperBytes =
        Messages.Response.newBuilder().setData(response.toByteString()).build().toByteArray();
    return transfer(wrapperBytes);
  }

  private int transferResponseError(String error) {
    final byte[] wrapperBytes =
        Messages.Response.newBuilder().setError(error).build().toByteArray();
    return transfer(wrapperBytes);
  }

  private byte[] consume(int addr) {
    final Memory mem = instance.memory();
    final int len = (int) (mem.readU32(addr - 4) - 4L);
    final byte[] data = mem.readBytes(addr, len);
    wasmMsgFree.apply(addr);
    return data;
  }

  private int transfer(byte[] data) {
    final Memory mem = instance.memory();
    final int addr = (int) wasmMsgAlloc.apply(data.length)[0];
    mem.write(addr, data);
    return addr;
  }

  private <T extends Message> ImportFunction createImportFunction(
      String name, ParserFn<T> reqCodec, Function<T, Message> impl) {
    return new ImportFunction(
        "wasm_msg",
        "wasm_msg_host_" + name,
        HOST_FN_TYPE,
        (instance1, args) -> {
          try {
            final T message = consumeRequest((int) args[0], reqCodec);
            final Message response = impl.apply(message);
            return new long[] {transferResponseSuccess(response)};
          } catch (Exception e) {
            return new long[] {transferResponseError(e.getMessage())};
          }
        });
  }

  private interface ParserFn<T> {

    T apply(byte[] data) throws InvalidProtocolBufferException;
  }
}
