import { describe, expect, it } from 'vitest';
import { WasmResolver } from './WasmResolver';
import { unpackSlice } from './slice';
import { createRecordingHost, guestLayout, guestModuleWithoutExports, loadFixture } from './test-helpers';

const fakeGuestModule = new WebAssembly.Module(loadFixture('fake-guest.wasm'));
const bareGuestModule = new WebAssembly.Module(loadFixture('bare-guest.wasm'));

function sharedMemory(): WebAssembly.Memory {
  return new WebAssembly.Memory({ initial: 1, maximum: 16, shared: true });
}

const FAKELOGS = [0x46, 0x41, 0x4b, 0x45, 0x4c, 0x4f, 0x47, 0x53]; // "FAKELOGS"

describe('WasmResolver construction', () => {
  it('rejects a module missing required exports', () => {
    expect(
      () =>
        new WasmResolver(
          new WebAssembly.Module(guestModuleWithoutExports()),
          sharedMemory(),
          createRecordingHost().host,
        ),
    ).toThrow(/missing required export "alloc"/);
  });

  it('rejects a non-shared memory', () => {
    const memory = new WebAssembly.Memory({ initial: 1, maximum: 16 });
    expect(() => new WasmResolver(fakeGuestModule, memory, createRecordingHost().host)).toThrow(/must be shared/);
  });

  it('detects optional exports', () => {
    const memory = sharedMemory();
    const host = createRecordingHost();
    expect(new WasmResolver(fakeGuestModule, memory, host.host).capabilities).toEqual({
      prometheusSnapshot: true,
      observeClientResolveLatency: true,
      evaluate: true,
    });
    expect(new WasmResolver(bareGuestModule, sharedMemory(), createRecordingHost().host).capabilities).toEqual({
      prometheusSnapshot: false,
      observeClientResolveLatency: false,
      evaluate: false,
    });
  });

  it('calls random_seed once per instance and records it in shared memory', () => {
    const memory = sharedMemory();
    const host = createRecordingHost();
    new WasmResolver(fakeGuestModule, memory, host.host);
    new WasmResolver(fakeGuestModule, memory, host.host);
    expect(host.randomSeeds).toHaveLength(2);
    expect(guestLayout(memory).lastSeed()).toBe(host.randomSeeds[1]);
  });
});

describe('WasmResolver slices', () => {
  it('round-trips bytes and tracks 8-byte alignment', () => {
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), createRecordingHost().host);
    const slice = gi.writeSlice(new Uint8Array([1, 2, 3, 4, 5]));
    expect([...gi.readSlice(slice)]).toEqual([1, 2, 3, 4, 5]);
    expect(gi.allocatedBytes()).toBe(5);
    gi.writeSlice(new Uint8Array([6, 7, 8]));
    expect(gi.allocatedBytes()).toBe(11); // 5 + 3 aligned up to 8
  });

  it('returns the absent slice for empty input', () => {
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), createRecordingHost().host);
    expect(gi.writeSlice(new Uint8Array(0))).toBe(0n);
  });

  it('surfaces allocation failure', () => {
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), createRecordingHost().host);
    expect(() => gi.writeSlice(new Uint8Array(100_000))).toThrow(/alloc\(100000\) failed/);
  });

  it('shares the arena across instances of the same memory', () => {
    const memory = sharedMemory();
    const host = createRecordingHost();
    const a = new WasmResolver(fakeGuestModule, memory, host.host);
    const b = new WasmResolver(fakeGuestModule, memory, host.host);
    a.writeSlice(new Uint8Array(8));
    const topAfterA = guestLayout(memory).bumpTop();
    b.writeSlice(new Uint8Array(8));
    expect(guestLayout(memory).bumpTop()).toBe(topAfterA + 8);
    expect(b.allocatedBytes()).toBe(16); // b's view includes a's allocation
  });
});

describe('WasmResolver ABI methods', () => {
  it('resolveProcessStart echoes the request; evaluate borrows the envelope repeatedly', () => {
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), createRecordingHost().host);
    const request = new Uint8Array([9, 8, 7, 6, 5, 4, 3, 250]);
    const envelope = gi.resolveProcessStart(request);
    expect([...gi.readSlice(envelope)]).toEqual([...request]);

    // the envelope is borrowed: evaluate against it as many times as needed
    const first = new Uint8Array([1, 1, 1]);
    const second = new Uint8Array([2, 2, 2, 2]);
    expect([...gi.evaluate(envelope, first)]).toEqual([...first]);
    expect([...gi.evaluate(envelope, second)]).toEqual([...second]);

    gi.free(unpackSlice(envelope).ptr); // freed once, after the last evaluate
  });

  it('applyFlags returns 0 and consults the apply filter; 1 when the fingerprint is seen', () => {
    const host = createRecordingHost();
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), host.host);
    const request = new Uint8Array(5);
    expect(gi.applyFlags(request)).toBe(0);
    expect(host.applySeenQueries).toEqual([5n]);
    host.applySeen.add(5n);
    expect(gi.applyFlags(new Uint8Array(5))).toBe(1);
  });

  it('flushLogs delivers a payload through write_logs and the ack is guaranteed', () => {
    const host = createRecordingHost();
    const memory = sharedMemory();
    const gi = new WasmResolver(fakeGuestModule, memory, host.host);

    gi.flushLogs();
    expect(host.writeLogPayloads).toHaveLength(1);
    const payload = host.writeLogPayloads[0];
    expect(payload).toHaveLength(16);
    expect([...payload.slice(8, 16)]).toEqual(FAKELOGS);
    expect(new DataView(payload.buffer, payload.byteOffset).getBigUint64(0, true)).toBe(host.currentTimes[0]);
    // the host implementation never acked: WasmResolver's finally did, with the same slice
    const layout = guestLayout(memory);
    expect(layout.lastAck()).toBe(layout.lastWriteLogs());
    expect(layout.lastAck()).not.toBe(0n);
  });

  it('swallows a throwing write_logs, reports it, and still acks', () => {
    const host = createRecordingHost();
    host.writeLogsThrows = true;
    const memory = sharedMemory();
    const gi = new WasmResolver(fakeGuestModule, memory, host.host);

    expect(() => gi.flushLogs()).not.toThrow();
    expect(host.hostErrors).toEqual([{ where: 'writeLogs', error: new Error('boom from writeLogs') }]);
    const layout = guestLayout(memory);
    expect(layout.lastAck()).toBe(layout.lastWriteLogs());
  });

  it('acks exactly once even when the host callback also acks', () => {
    const host = createRecordingHost();
    host.writeLogsAcks = true;
    const memory = sharedMemory();
    const gi = new WasmResolver(fakeGuestModule, memory, host.host);
    gi.flushLogs();
    const layout = guestLayout(memory);
    expect(layout.lastAck()).toBe(layout.lastWriteLogs());
  });

  it('prometheusSnapshot decodes text; destroy increments the shared counter', () => {
    const memory = sharedMemory();
    const host = createRecordingHost();
    const gi = new WasmResolver(fakeGuestModule, memory, host.host);
    expect(new TextDecoder().decode(gi.prometheusSnapshot(0))).toBe('fake-metrics');

    gi.destroyInstance();
    expect(guestLayout(memory).destroyCount()).toBe(1);
    new WasmResolver(fakeGuestModule, memory, host.host).destroyInstance();
    expect(guestLayout(memory).destroyCount()).toBe(2);
  });

  it('observeClientResolveLatency is a no-op when present, throws when absent', () => {
    const gi = new WasmResolver(fakeGuestModule, sharedMemory(), createRecordingHost().host);
    expect(() => gi.observeClientResolveLatency(3, 1500n)).not.toThrow();
    const bare = new WasmResolver(bareGuestModule, sharedMemory(), createRecordingHost().host);
    expect(() => bare.observeClientResolveLatency(3, 1500n)).toThrow(
      /does not export "observe_client_resolve_latency"/,
    );
    expect(() => bare.prometheusSnapshot(0)).toThrow(/does not export "metrics_snapshot"/);
    expect(() => bare.evaluate(0n, new Uint8Array([1]))).toThrow(/does not export "evaluate"/);
  });
});
