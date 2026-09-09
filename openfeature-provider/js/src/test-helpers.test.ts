import { afterEach, describe, expect, it, vi } from 'vitest';
import { advanceTimersUntil, encryptTestState, useFakeTimerCompatibleCrypto } from './test-helpers';

describe('fake-timer crypto adapter', () => {
  useFakeTimerCompatibleCrypto();
  afterEach(() => vi.useRealTimers());

  it('decrypts before advancing a fake deadline and still authenticates ciphertext', async () => {
    const key = await crypto.subtle.importKey('raw', new Uint8Array(32), 'AES-GCM', false, ['decrypt']);
    const plaintext = new Uint8Array([1, 2, 3]);
    const encrypted = encryptTestState(plaintext);
    vi.useFakeTimers();
    const deadline = vi.fn();
    setTimeout(deadline, 1);
    const decrypt = () =>
      crypto.subtle.decrypt({ name: 'AES-GCM', iv: encrypted.subarray(0, 12) }, key, encrypted.subarray(12));
    expect(new Uint8Array(await advanceTimersUntil(decrypt()))).toEqual(plaintext);
    expect(deadline).not.toHaveBeenCalled();
    encrypted[encrypted.length - 1] ^= 1;
    await expect(advanceTimersUntil(decrypt())).rejects.toThrow();
    expect(deadline).not.toHaveBeenCalled();
    vi.clearAllTimers();
  });

  it('delegates to real WebCrypto with real timers', async () => {
    const key = await crypto.subtle.importKey('raw', new Uint8Array(32), 'AES-GCM', false, ['decrypt']);
    const encrypted = encryptTestState(new Uint8Array([42]));
    expect(
      new Uint8Array(
        await crypto.subtle.decrypt({ name: 'AES-GCM', iv: encrypted.subarray(0, 12) }, key, encrypted.subarray(12)),
      ),
    ).toEqual(new Uint8Array([42]));
    encrypted[encrypted.length - 1] ^= 1;
    await expect(
      crypto.subtle.decrypt({ name: 'AES-GCM', iv: encrypted.subarray(0, 12) }, key, encrypted.subarray(12)),
    ).rejects.toMatchObject({ name: 'OperationError' });
  });
});
