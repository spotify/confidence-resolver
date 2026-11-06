import { describe, expect, it } from 'vitest';
import { WasmResolverPool, type WasmResolverPoolOptions } from './instance-pool';
import type { WasmResolver } from './WasmResolver';
import {
  createRecordingHost,
  guestLayout,
  guestModuleWithoutExports,
  loadFixture,
  moduleWithoutHostMemory,
  type RecordingHost,
} from './test-helpers';

const fakeGuestBytes = loadFixture('fake-guest.wasm');

async function createPool(
  options?: WasmResolverPoolOptions,
  host: RecordingHost = createRecordingHost(),
): Promise<{ pool: WasmResolverPool; host: RecordingHost }> {
  const pool = await WasmResolverPool.create(fakeGuestBytes, host.host, options);
  return { pool, host };
}

describe('WasmResolverPool creation', () => {
  it('warms one instance by default', async () => {
    const { pool, host } = await createPool();
    expect(pool.liveCount).toBe(1);
    expect(pool.heldCount).toBe(0);
    expect(host.randomSeeds).toHaveLength(1); // random_seed runs per instance, at instantiation
  });

  it('rejects a module that does not import host.memory', async () => {
    const host = createRecordingHost();
    await expect(WasmResolverPool.create(moduleWithoutHostMemory(), host.host)).rejects.toThrow(
      /does not import host\.memory/,
    );
  });

  it('rejects a module missing required exports at warm-up', async () => {
    const host = createRecordingHost();
    await expect(WasmResolverPool.create(guestModuleWithoutExports(), host.host)).rejects.toThrow(
      /missing required export/,
    );
  });

  it('validates options', async () => {
    await expect(createPool({ maxInstances: 0 })).rejects.toThrow(/maxInstances/);
    await expect(createPool({ warmInstances: 3, maxInstances: 2 })).rejects.toThrow(/warmInstances/);
  });
});

describe('WasmResolverPool leasing', () => {
  it('reuses released instances', async () => {
    const { pool, host } = await createPool();
    const a = await pool.acquire();
    expect(pool.heldCount).toBe(1);
    pool.release(a);
    expect(pool.heldCount).toBe(0);
    const b = await pool.acquire();
    expect(b).toBe(a); // same instance object, no re-instantiation
    expect(host.randomSeeds).toHaveLength(1);
    pool.release(b);
  });

  it('gives concurrent holders distinct instances', async () => {
    const { pool, host } = await createPool();
    const a = await pool.acquire();
    const b = await pool.acquire();
    expect(a).not.toBe(b);
    expect(pool.liveCount).toBe(2);
    expect(host.randomSeeds).toHaveLength(2); // warm(1) + one new
    pool.release(a);
    pool.release(b);
  });

  it('caps live instances and hands released instances to waiters', async () => {
    const { pool } = await createPool({ maxInstances: 2, warmInstances: 1 });
    const a = await pool.acquire(); // the warmed instance
    const b = await pool.acquire(); // creates the second (at cap)
    expect(pool.liveCount).toBe(2);

    let acquired: WasmResolver | undefined;
    let settled = false;
    const third = pool.acquire().then(instance => {
      settled = true;
      return instance;
    });
    await new Promise(resolve => setImmediate(resolve)); // let the microtask queue drain
    expect(settled).toBe(false); // parked at the cap

    pool.release(a);
    acquired = await third;
    expect(acquired).toBe(a); // the released instance is handed over, not a new one
    expect(pool.liveCount).toBe(2);
    expect(pool.heldCount).toBe(2);
    pool.release(b);
    pool.release(acquired);
  });

  it('rejects double release', async () => {
    const { pool } = await createPool();
    const a = await pool.acquire();
    pool.release(a);
    expect(() => pool.release(a)).toThrow(/not held/);
  });

  it('flushes logs on an idle instance when one exists, and skips otherwise', async () => {
    const { pool, host } = await createPool();
    expect(pool.tryFlushLogsIdle()).toBe(true);
    expect(host.writeLogPayloads).toHaveLength(1);
    expect([...host.writeLogPayloads[0].slice(8, 16)]).toEqual([0x46, 0x41, 0x4b, 0x45, 0x4c, 0x4f, 0x47, 0x53]);

    const a = await pool.acquire(); // no idle instance left
    expect(pool.tryFlushLogsIdle()).toBe(false);
    expect(host.writeLogPayloads).toHaveLength(1);
    pool.release(a);
    expect(pool.tryFlushLogsIdle()).toBe(true);
    expect(host.writeLogPayloads).toHaveLength(2);
  });
});

describe('WasmResolverPool destruction', () => {
  it('destroys idle instances and rejects new acquires', async () => {
    const { pool } = await createPool();
    await pool.destroy();
    expect(pool.liveCount).toBe(0);
    expect(guestLayout(pool.memory).destroyCount()).toBe(1); // the warmed instance
    await expect(pool.acquire()).rejects.toThrow(/destroyed/);
    await expect(pool.destroy()).resolves.toBeUndefined(); // idempotent
  });

  it('waits for held instances, then destroys them', async () => {
    const { pool } = await createPool();
    const a = await pool.acquire();
    let destroyed = false;
    const destroying = pool.destroy().then(() => {
      destroyed = true;
    });
    await new Promise(resolve => setImmediate(resolve));
    expect(destroyed).toBe(false); // still waiting for the holder

    pool.release(a);
    await destroying;
    expect(destroyed).toBe(true);
    expect(guestLayout(pool.memory).destroyCount()).toBe(1); // warm + held = 1 instance total
    expect(pool.liveCount).toBe(0);
  });

  it('destroys instances released after close, without handing them to waiters', async () => {
    const { pool } = await createPool({ maxInstances: 1, warmInstances: 1 });
    const a = await pool.acquire();
    let rejected: Error | undefined;
    const queued = pool.acquire().catch((error: Error) => {
      rejected = error;
    });
    const destroying = pool.destroy();
    pool.release(a);
    await Promise.all([queued, destroying]);
    expect(rejected?.message).toMatch(/destroyed while waiting/);
    expect(guestLayout(pool.memory).destroyCount()).toBe(1);
  });
});
