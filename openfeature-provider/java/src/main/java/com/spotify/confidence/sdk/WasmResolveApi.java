package com.spotify.confidence.sdk;

import com.dylibso.chicory.compiler.MachineFactoryCompiler;
import com.dylibso.chicory.runtime.ExportFunction;
import com.dylibso.chicory.runtime.ImportFunction;
import com.dylibso.chicory.runtime.ImportValues;
import com.dylibso.chicory.runtime.Instance;
import com.dylibso.chicory.runtime.Memory;
import com.dylibso.chicory.wasm.Parser;
import com.dylibso.chicory.wasm.WasmModule;
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
import java.io.IOException;
import java.io.InputStream;
import java.time.Instant;
import java.util.List;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;
import java.util.function.Function;

class WasmResolveApi {
  private final FunctionType HOST_FN_TYPE =
      FunctionType.of(List.of(ValType.I32), List.of(ValType.I32));
  private final Instance instance;
  private boolean isConsumed = false;

  // interop
  private final ExportFunction wasmMsgAlloc;
  private final ExportFunction wasmMsgFree;
  private final WasmFlagLogger writeFlagLogs;

  // api
  private final ExportFunction wasmMsgGuestSetResolverState;
  private final ExportFunction wasmMsgFlushLogs;
  private final ExportFunction wasmMsgGuestApplyFlags;
  private final ExportFunction wasmMsgGuestResolveProcess;
  private final ReadWriteLock wasmLock = new ReentrantReadWriteLock();

  public WasmResolveApi(WasmFlagLogger flagLogger) {
    this.writeFlagLogs = flagLogger;
    try (InputStream wasmStream =
        getClass().getClassLoader().getResourceAsStream("wasm/confidence_resolver.wasm")) {
      if (wasmStream == null) {
        throw new RuntimeException("Could not find confidence_resolver.wasm in resources");
      }
      final WasmModule module = Parser.parse(wasmStream);
      instance =
          Instance.builder(module)
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
              .withMachineFactory(MachineFactoryCompiler::compile)
              .build();
      wasmMsgAlloc = instance.export("wasm_msg_alloc");
      wasmMsgFree = instance.export("wasm_msg_free");
      wasmMsgGuestSetResolverState = instance.export("wasm_msg_guest_set_resolver_state");
      wasmMsgFlushLogs = instance.export("wasm_msg_guest_flush_logs");
      wasmMsgGuestApplyFlags = instance.export("wasm_msg_guest_apply_flags");
      wasmMsgGuestResolveProcess = instance.export("wasm_msg_guest_resolve_flags");
    } catch (IOException e) {
      throw new RuntimeException("Failed to load WASM module", e);
    }
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

  public void setResolverState(byte[] state, String accountId) {
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
  }

  public void close() {
    wasmLock.readLock().lock();
    try {
      final var voidRequest = Messages.Void.getDefaultInstance();
      final var reqPtr = transferRequest(voidRequest);
      final var respPtr = (int) wasmMsgFlushLogs.apply(reqPtr)[0];
      final var request = consumeResponse(respPtr, WriteFlagLogsRequest::parseFrom);
      writeFlagLogs.writeSync(request);
      isConsumed = true;
    } finally {
      wasmLock.readLock().unlock();
    }
  }

  public ResolveProcessResponse resolveProcess(ResolveProcessRequest request)
      throws IsClosedException {
    if (!wasmLock.writeLock().tryLock()) {
      throw new IsClosedException();
    }
    try {
      if (isConsumed) {
        throw new IsClosedException();
      }
      final int reqPtr = transferRequest(request);
      final int respPtr = (int) wasmMsgGuestResolveProcess.apply(reqPtr)[0];
      return consumeResponse(respPtr, ResolveProcessResponse::parseFrom);
    } finally {
      wasmLock.writeLock().unlock();
    }
  }

  public void applyFlags(ApplyFlagsRequest request) throws IsClosedException {
    if (!wasmLock.writeLock().tryLock()) {
      throw new IsClosedException();
    }
    try {
      if (isConsumed) {
        throw new IsClosedException();
      }
      final int reqPtr = transferRequest(request);
      final int respPtr = (int) wasmMsgGuestApplyFlags.apply(reqPtr)[0];
      consumeResponse(respPtr, ApplyFlagsResponse::parseFrom);
    } finally {
      wasmLock.writeLock().unlock();
    }
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
