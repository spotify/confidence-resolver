import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { publishFlagEvaluation } from './flag-evaluation-global';

describe('publishFlagEvaluation', () => {
  let originalWindow: typeof globalThis.window;

  beforeEach(() => {
    originalWindow = globalThis.window;
  });

  afterEach(() => {
    if (originalWindow === undefined) {
      // @ts-expect-error restoring undefined
      delete globalThis.window;
    } else {
      globalThis.window = originalWindow;
    }
  });

  it('writes { variant, assignmentOrigin } to window.__confidence.flags', () => {
    globalThis.window = {} as any;

    publishFlagEvaluation('flags/my-flag', 'flags/my-flag/variants/treatment', 'rule-1');

    expect((window as any).__confidence.flags['flags/my-flag']).toEqual({
      variant: 'flags/my-flag/variants/treatment',
      assignmentOrigin: 'rule-1',
    });
  });

  it('initializes __confidence and flags if missing', () => {
    globalThis.window = {} as any;

    publishFlagEvaluation('flags/test', 'flags/test/variants/control', '');

    expect((window as any).__confidence).toBeDefined();
    expect((window as any).__confidence.flags).toBeDefined();
    expect((window as any).__confidence.flags['flags/test']).toEqual({
      variant: 'flags/test/variants/control',
      assignmentOrigin: '',
    });
  });

  it('preserves existing __confidence object', () => {
    const existing = { other: 'data' };
    globalThis.window = { __confidence: existing } as any;

    publishFlagEvaluation('flags/my-flag', 'flags/my-flag/variants/treatment', 'rule-1');

    expect((window as any).__confidence.other).toBe('data');
    expect((window as any).__confidence.flags['flags/my-flag']).toEqual({
      variant: 'flags/my-flag/variants/treatment',
      assignmentOrigin: 'rule-1',
    });
  });

  it('preserves existing flags object (e.g. recorder Proxy)', () => {
    const flags = { 'flags/existing': { variant: 'flags/existing/variants/a', assignmentOrigin: '' } };
    globalThis.window = { __confidence: { flags } } as any;

    publishFlagEvaluation('flags/new-flag', 'flags/new-flag/variants/b', 'rule-2');

    expect((window as any).__confidence.flags['flags/existing']).toEqual({
      variant: 'flags/existing/variants/a',
      assignmentOrigin: '',
    });
    expect((window as any).__confidence.flags['flags/new-flag']).toEqual({
      variant: 'flags/new-flag/variants/b',
      assignmentOrigin: 'rule-2',
    });
    expect((window as any).__confidence.flags).toBe(flags);
  });

  it('deduplicates: skips write when variant unchanged', () => {
    const flags: Record<string, { variant: string; assignmentOrigin: string }> = {};
    globalThis.window = { __confidence: { flags } } as any;

    Object.defineProperty(flags, 'flags/my-flag', {
      get: () => ({ variant: 'flags/my-flag/variants/treatment', assignmentOrigin: 'rule-1' }),
      set: vi.fn(),
      configurable: true,
    });

    publishFlagEvaluation('flags/my-flag', 'flags/my-flag/variants/treatment', 'rule-1');

    expect(vi.mocked(Object.getOwnPropertyDescriptor(flags, 'flags/my-flag')!.set!)).not.toHaveBeenCalled();
  });

  it('writes when variant changes', () => {
    globalThis.window = {
      __confidence: {
        flags: { 'flags/my-flag': { variant: 'flags/my-flag/variants/control', assignmentOrigin: '' } },
      },
    } as any;

    publishFlagEvaluation('flags/my-flag', 'flags/my-flag/variants/treatment', 'rule-1');

    expect((window as any).__confidence.flags['flags/my-flag']).toEqual({
      variant: 'flags/my-flag/variants/treatment',
      assignmentOrigin: 'rule-1',
    });
  });

  it('is a no-op when window is undefined', () => {
    // @ts-expect-error simulating server-side
    delete globalThis.window;

    expect(() => {
      publishFlagEvaluation('flags/my-flag', 'flags/my-flag/variants/treatment', '');
    }).not.toThrow();
  });
});
