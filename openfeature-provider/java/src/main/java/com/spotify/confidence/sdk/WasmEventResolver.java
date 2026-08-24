package com.spotify.confidence.sdk;

import com.dylibso.chicory.runtime.ChicoryInterruptedException;
import com.dylibso.chicory.runtime.ExportFunction;
import com.dylibso.chicory.runtime.Instance;
import com.dylibso.chicory.runtime.Memory;
import com.dylibso.chicory.wasm.ChicoryException;
import com.dylibso.chicory.wasm.WasmModule;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import com.spotify.confidence.sdk.events.wasm.v1.FlushEventsResponse;
import com.spotify.confidence.sdk.events.wasm.v1.TrackEventRequest;
import com.spotify.confidence.sdk.wasm.Messages;
import java.util.concurrent.locks.ReentrantLock;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * WASM wrapper for the Confidence event engine. Loads the event engine WASM binary and exposes
 * {@link #trackEvent(TrackEventRequest)} and {@link #flushEvents()} operations.
 *
 * <p>The event engine WASM has no host imports (unlike the flag resolver WASM). It uses the same
 * wasm-msg protocol: alloc memory, write a {@code Request} protobuf envelope, call the WASM export,
 * read the {@code Response} envelope from the returned pointer, and free.
 *
 * <p>Thread-safe via {@link ReentrantLock}.
 *
 * <p>If the WASM instance traps, it is rebuilt from the parsed module so subsequent calls run
 * against a fresh instance (buffered events in the trapped instance are lost). This mirrors {@link
 * RecoveringResolver} for the flag resolver and {@code EventTracker.reloadLocked} in the Go
 * provider. Only genuine WASM faults trigger a reload — protobuf decoding failures and errors
 * reported by the engine in the response envelope leave the instance untouched.
 */
class WasmEventResolver implements AutoCloseable {
  private static final Logger logger = LoggerFactory.getLogger(WasmEventResolver.class);

  private final WasmModule module;
  private final ReentrantLock lock = new ReentrantLock();
  private Instance instance;
  private ExportFunction wasmMsgAlloc;
  private ExportFunction wasmMsgFree;
  private ExportFunction wasmMsgGuestTrackEvent;
  private ExportFunction wasmMsgGuestBoundedFlushEvents;
  private boolean closed = false;

  WasmEventResolver(byte[] wasmBytes) {
    this.module = com.dylibso.chicory.wasm.Parser.parse(wasmBytes);
    instantiate();
  }

  /** Builds a fresh instance from {@link #module} and rebinds the exported functions. */
  private void instantiate() {
    this.instance = Instance.builder(module).build();
    this.wasmMsgAlloc = instance.export("wasm_msg_alloc");
    this.wasmMsgFree = instance.export("wasm_msg_free");
    this.wasmMsgGuestTrackEvent = instance.export("wasm_msg_guest_track_event");
    this.wasmMsgGuestBoundedFlushEvents = instance.export("wasm_msg_guest_bounded_flush_events");
  }

  /**
   * Tracks an event by sending a {@link TrackEventRequest} to the event engine WASM. The event is
   * buffered internally until {@link #flushEvents()} is called.
   */
  void trackEvent(TrackEventRequest request) {
    lock.lock();
    try {
      if (closed || instance == null) {
        return;
      }
      try {
        final int reqPtr = transferRequest(request);
        final int respPtr = (int) wasmMsgGuestTrackEvent.apply(reqPtr)[0];
        consumeVoidResponse(respPtr);
      } catch (ChicoryException e) {
        handleTrapLocked("trackEvent", e);
        throw e;
      }
    } finally {
      lock.unlock();
    }
  }

  /**
   * Flushes buffered events from the WASM engine, returning a {@link FlushEventsResponse}. The
   * returned batch may contain zero events if nothing was buffered.
   *
   * <p>This is a bounded flush: multiple calls may be needed to drain all events.
   */
  FlushEventsResponse flushEvents() {
    lock.lock();
    try {
      if (closed || instance == null) {
        return FlushEventsResponse.getDefaultInstance();
      }
      try {
        // The event engine WASM expects no input for flush (matching JS reference: passes 0)
        final int respPtr = (int) wasmMsgGuestBoundedFlushEvents.apply(0)[0];
        final FlushEventsResponse response =
            consumeTypedResponse(respPtr, FlushEventsResponse::parseFrom);
        // consumeTypedResponse yields null when the guest returned no response.
        return response != null ? response : FlushEventsResponse.getDefaultInstance();
      } catch (ChicoryException e) {
        handleTrapLocked("flushEvents", e);
        throw e;
      }
    } finally {
      lock.unlock();
    }
  }

  /**
   * Replaces a trapped WASM instance with a fresh one. Buffered events in the old instance are
   * lost. Caller must hold {@link #lock}.
   */
  private void handleTrapLocked(String opName, ChicoryException e) {
    if (e instanceof ChicoryInterruptedException) {
      logger.debug("Event engine interrupted during {}, not reloading", opName);
      return;
    }
    if (closed) {
      return;
    }
    logger.warn(
        "Event engine WASM failed during {} ({}), reloading instance; buffered events are lost",
        opName,
        e.getMessage(),
        e);
    instance = null;
    try {
      instantiate();
    } catch (RuntimeException reloadError) {
      // Leave instance null — subsequent calls become no-ops until close().
      instance = null;
      logger.error("Failed to reload the event engine WASM instance", reloadError);
    }
  }

  @Override
  public void close() {
    lock.lock();
    try {
      closed = true;
    } finally {
      lock.unlock();
    }
  }

  private int transferRequest(Message message) {
    final byte[] request =
        Messages.Request.newBuilder().setData(message.toByteString()).build().toByteArray();
    return transfer(request);
  }

  private void consumeVoidResponse(int addr) {
    // See consumeTypedResponse: addr == 0 means no response to consume.
    if (addr == 0) {
      return;
    }
    try {
      final Messages.Response response = Messages.Response.parseFrom(consume(addr));
      if (response.hasError()) {
        throw new RuntimeException("Event WASM error: " + response.getError());
      }
    } catch (InvalidProtocolBufferException e) {
      throw new RuntimeException(e);
    }
  }

  private <T> T consumeTypedResponse(int addr, ParserFn<T> codec) {
    // A null pointer means the guest produced no response. Falling through would
    // make consume() read the length prefix at addr-4, i.e. 0xFFFFFFFC, trapping
    // or returning garbage. Go guards the same way (`if resPtr[0] == 0`).
    if (addr == 0) {
      return null;
    }
    try {
      final Messages.Response response = Messages.Response.parseFrom(consume(addr));
      if (response.hasError()) {
        throw new RuntimeException("Event WASM error: " + response.getError());
      }
      return codec.apply(response.getData().toByteArray());
    } catch (InvalidProtocolBufferException e) {
      throw new RuntimeException(e);
    }
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

  @FunctionalInterface
  private interface ParserFn<T> {
    T apply(byte[] data) throws InvalidProtocolBufferException;
  }
}
