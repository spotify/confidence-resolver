import { logger } from './logger';

/**
 * Log a warning message only in non-production environments.
 */
export function devWarn(message: string): void {
  if (typeof process !== 'undefined' && process.env?.NODE_ENV !== 'production') {
    console.warn(message);
  }
}

export const enum TimeUnit {
  MILLISECOND = 1,
  SECOND = 1000,
  MINUTE = 1000 * 60,
  HOUR = 1000 * 60 * 60,
  DAY = 1000 * 60 * 60 * 24,
}
export function scheduleWithFixedDelay(operation: (signal?: AbortSignal) => unknown, delayMs: number): () => void {
  const ac = new AbortController();
  let nextRunTimeoutId = 0;

  const run = async () => {
    try {
      await operation(ac.signal);
    } catch (e: unknown) {
      logger.warn('scheduleWithFixedDelay failure:', e);
    }
    nextRunTimeoutId = portableSetTimeout(run, delayMs);
  };

  nextRunTimeoutId = portableSetTimeout(run, delayMs);
  return () => {
    clearTimeout(nextRunTimeoutId);
    ac.abort();
  };
}

export function scheduleWithFixedInterval(
  operation: (signal?: AbortSignal) => unknown,
  intervalMs: number,
  opt: { maxConcurrent?: number; signal?: AbortSignal } = {},
): () => void {
  const maxConcurrent = opt.maxConcurrent ?? 1;
  const ac = new AbortController();
  let nextRunTimeoutId = 0;
  let lastRunTime = 0;
  let concurrent = 0;

  if (__ASSERT__) {
    if (!Number.isInteger(maxConcurrent) || maxConcurrent < 1) {
      throw new Error(`maxConcurrent must be an integer greater than zero, was ${maxConcurrent}`);
    }
  }

  const run = async () => {
    lastRunTime = Date.now();
    nextRunTimeoutId = portableSetTimeout(run, intervalMs);
    if (concurrent >= maxConcurrent) {
      return;
    }
    concurrent++;
    try {
      await operation(ac.signal);
    } catch (e: unknown) {
      logger.warn('scheduleWithFixedInterval failure:', e);
    }
    concurrent--;
    const timeSinceLast = Date.now() - lastRunTime;
    if (timeSinceLast > intervalMs && nextRunTimeoutId != 0) {
      clearTimeout(nextRunTimeoutId);
      run();
    }
  };

  nextRunTimeoutId = portableSetTimeout(run, intervalMs);

  const stop = () => {
    clearTimeout(nextRunTimeoutId);
    nextRunTimeoutId = 0;
    ac.abort();
  };
  opt.signal?.addEventListener('abort', stop);
  return stop;
}

export function timeoutSignal(milliseconds: number): AbortSignal {
  const ac = new AbortController();
  portableSetTimeout(
    () => {
      ac.abort(new Error('Timeout'));
      // ac.abort('Timeout');
    },
    milliseconds,
    { unref: false },
  );
  return ac.signal;
}

export function portableSetTimeout(callback: () => void, milliseconds: number, opt: { unref?: boolean } = {}): number {
  const timeout: unknown = setTimeout(callback, milliseconds);
  if (
    opt.unref &&
    typeof timeout === 'object' &&
    timeout !== null &&
    'unref' in timeout &&
    typeof timeout.unref === 'function'
  ) {
    timeout.unref();
  }
  return Number(timeout);
}

export function abortableSleep(milliseconds: number, signal?: AbortSignal | null): Promise<void> {
  if (signal?.aborted) {
    return Promise.reject(signal.reason);
  }
  if (milliseconds <= 0) {
    return Promise.resolve();
  }
  if (milliseconds === Infinity) {
    return signal ? promiseSignal(signal) : new Promise(() => {});
  }
  return new Promise((resolve, reject) => {
    let timeout: number;
    const onTimeout = () => {
      cleanup();
      resolve();
    };
    const onAbort = () => {
      cleanup();
      reject(signal?.reason);
    };
    const cleanup = () => {
      clearTimeout(timeout);
      signal?.removeEventListener('abort', onAbort);
    };

    signal?.addEventListener('abort', onAbort);
    timeout = portableSetTimeout(onTimeout, milliseconds);
  });
}

export function promiseSignal(signal: AbortSignal): Promise<never> {
  if (signal.aborted) {
    return Promise.reject(signal.reason);
  }
  return new Promise((_, reject) => {
    signal.addEventListener(
      'abort',
      () => {
        reject(signal.reason);
      },
      { once: true },
    );
  });
}

export function abortablePromise<T>(promise: Promise<T>, signal?: AbortSignal | null | undefined): Promise<T> {
  return signal ? Promise.race([promise, promiseSignal(signal)]) : promise;
}

export function isObject(value: unknown): value is {} {
  return typeof value === 'object' && value !== null;
}

export function hasKey<K extends string>(obj: object, key: K): obj is { [P in K]: unknown } {
  return key in obj;
}

export function castStringToEnum<E extends string>(value: `${E}`): E {
  return value as E;
}

/**
 * Decode a base64 string to Uint8Array.
 * Works in both Node.js (using Buffer) and browsers (using atob).
 */
export function bytesFromBase64(b64: string): Uint8Array {
  if ((globalThis as any).Buffer) {
    return Uint8Array.from(globalThis.Buffer.from(b64, 'base64'));
  } else {
    const bin = globalThis.atob(b64);
    const arr = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; ++i) {
      arr[i] = bin.charCodeAt(i);
    }
    return arr;
  }
}

/**
 * Encode a Uint8Array to base64 string.
 * Works in both Node.js (using Buffer) and browsers (using btoa).
 */
export function base64FromBytes(arr: Uint8Array): string {
  if ((globalThis as any).Buffer) {
    return globalThis.Buffer.from(arr).toString('base64');
  } else {
    const bin: string[] = [];
    arr.forEach(byte => {
      bin.push(String.fromCharCode(byte));
    });
    return globalThis.btoa(bin.join(''));
  }
}
