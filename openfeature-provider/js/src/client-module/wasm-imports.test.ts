import { describe, expect, it } from 'vitest';
import { parseImports, readSharedMemoryLimits } from './wasm-imports';

// --- minimal wasm binary builder ------------------------------------------------

const WASM_HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

function uleb(value: number): number[] {
  const out: number[] = [];
  let v = value >>> 0;
  do {
    let byte = v & 0x7f;
    v >>>= 7;
    if (v !== 0) byte |= 0x80;
    out.push(byte);
  } while (v !== 0);
  return out;
}

function name(s: string): number[] {
  const bytes = [...new TextEncoder().encode(s)];
  return [...uleb(bytes.length), ...bytes];
}

function section(id: number, content: number[]): number[] {
  return [id, ...uleb(content.length), ...content];
}

function wasm(...sections: number[][]): Uint8Array {
  return new Uint8Array([...WASM_HEADER, ...sections.flat()]);
}

// import descriptors
const FUNC = (typeIndex: number) => [0x00, ...uleb(typeIndex)];
const TABLE = (initial: number) => [0x01, 0x70, 0x00, ...uleb(initial)];
const GLOBAL = () => [0x03, 0x7f, 0x00];
function MEMORY(flags: number, limits: number[]): number[] {
  return [0x02, flags, ...limits];
}
/** Shared with maximum: the contract's host.memory shape. */
function HOST_MEMORY(initial: number, maximum: number): number[] {
  return [...name('host'), ...name('memory'), ...MEMORY(0b11, [...uleb(initial), ...uleb(maximum)])];
}

function importSection(...entries: number[][]): number[] {
  return section(2, [...uleb(entries.length), ...entries.flat()]);
}

/** A realistic import set: host functions plus the shared host.memory. */
function moduleWithHostMemory(initial = 1, maximum = 16): Uint8Array {
  return wasm(
    importSection(
      [...name('env'), ...name('current_time'), ...FUNC(0)],
      [...name('env'), ...name('random_seed'), ...FUNC(0)],
      HOST_MEMORY(initial, maximum),
    ),
  );
}

// --- parseImports ---------------------------------------------------------------

describe('parseImports', () => {
  it('returns an empty list for a module without sections', () => {
    expect(parseImports(wasm())).toEqual([]);
  });

  it('returns an empty list when there is no import section', () => {
    // A custom section (id 0) plus a type section (id 1), no imports.
    const module = wasm(section(0, [...name('confidence.metadata'), 0xde, 0xad]), section(1, [...uleb(0)]));
    expect(parseImports(module)).toEqual([]);
  });

  it('skips sections before the import section', () => {
    const module = wasm(
      section(0, [...name('a-custom-section'), 0x01, 0x02]),
      section(1, [...uleb(2), ...FUNC(0), ...FUNC(0)]),
      importSection([...name('env'), ...name('write_logs'), ...FUNC(0)]),
    );
    expect(parseImports(module)).toEqual([{ module: 'env', field: 'write_logs', kind: 'function' }]);
  });

  it('parses every import kind', () => {
    const module = wasm(
      importSection(
        [...name('env'), ...name('current_time'), ...FUNC(3)],
        [...name('other'), ...name('a-table'), ...TABLE(4)],
        [...name('other'), ...name('a-global'), ...GLOBAL()],
        [...name('host'), ...name('memory'), ...MEMORY(0b01, [...uleb(2), ...uleb(8)])],
      ),
    );
    expect(parseImports(module)).toEqual([
      { module: 'env', field: 'current_time', kind: 'function' },
      { module: 'other', field: 'a-table', kind: 'table' },
      { module: 'other', field: 'a-global', kind: 'global' },
      { module: 'host', field: 'memory', kind: 'memory', memory: { initial: 2, maximum: 8, shared: false } },
    ]);
  });

  it('reports a non-shared memory and a memory without maximum as facts', () => {
    const module = wasm(
      importSection(
        [...name('a'), ...name('plain'), ...MEMORY(0b00, [...uleb(3)])],
        [...name('b'), ...name('shared-no-max'), ...MEMORY(0b10, [...uleb(3)])],
      ),
    );
    expect(parseImports(module)).toEqual([
      { module: 'a', field: 'plain', kind: 'memory', memory: { initial: 3, shared: false } },
      { module: 'b', field: 'shared-no-max', kind: 'memory', memory: { initial: 3, shared: true } },
    ]);
  });

  it('parses multi-byte uleb128 limits', () => {
    const module = wasm(importSection(HOST_MEMORY(128, 65536)));
    expect(parseImports(module)[0].memory).toEqual({ initial: 128, maximum: 65536, shared: true });
  });

  it('decodes multi-byte utf-8 names', () => {
    const module = wasm(importSection([...name(' módulé'), ...name('mémoire'), ...FUNC(0)]));
    expect(parseImports(module)).toEqual([{ module: ' módulé', field: 'mémoire', kind: 'function' }]);
  });

  it('does not match a function import named host.memory', () => {
    const module = wasm(importSection([...name('host'), ...name('memory'), ...FUNC(0)]));
    expect(parseImports(module)).toEqual([{ module: 'host', field: 'memory', kind: 'function' }]);
  });

  it('rejects a bad header', () => {
    expect(() => parseImports(new Uint8Array([0x00, 0x61, 0x73]))).toThrow(/shorter than the 8-byte header/);
    expect(() => parseImports(new Uint8Array([0x12, 0x34, 0x56, 0x78, 0x01, 0x00, 0x00, 0x00]))).toThrow(/bad magic/);
    expect(() => parseImports(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x02, 0x00, 0x00, 0x00]))).toThrow(
      /Unsupported wasm version/,
    );
  });

  it('rejects trailing bytes in the import section', () => {
    const module = wasm(section(2, [...uleb(1), ...HOST_MEMORY(1, 2), 0xff]));
    expect(() => parseImports(module)).toThrow(/trailing byte/);
  });

  it('rejects a section that extends past the end of the binary', () => {
    const module = new Uint8Array([...wasm(), 0x02, 0x80, 0x80, 0x80, 0x80, 0x0f /* huge size */]);
    expect(() => parseImports(module)).toThrow(/extends past the end/);
  });

  it('rejects a truncated import entry', () => {
    // Section promises 1 import; the module name declares 4 bytes but only 1 remains.
    const module = new Uint8Array([...WASM_HEADER, 0x02, 0x03, 0x01, 0x04, 0x68]);
    expect(() => parseImports(module)).toThrow(/Truncated/);
  });

  it('rejects malformed uleb128 values', () => {
    const malformed5thByte = wasm(
      importSection([...name('host'), ...name('memory'), 0x02, 0b11, 0x80, 0x80, 0x80, 0x80, 0x10]),
    );
    expect(() => parseImports(malformed5thByte)).toThrow(/exceeds 32 bits/);
    const sixBytes = wasm(
      importSection([...name('host'), ...name('memory'), 0x02, 0b11, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00]),
    );
    expect(() => parseImports(sixBytes)).toThrow(/runs past 32 bits/);
  });

  it('rejects unknown import kinds', () => {
    const module = wasm(importSection([...name('host'), ...name('mystery'), 0x05]));
    expect(() => parseImports(module)).toThrow(/Unknown import kind/);
  });

  it('rejects unsupported limit flags (e.g. 64-bit memories)', () => {
    const module = wasm(
      importSection([...name('host'), ...name('memory'), ...MEMORY(0b101, [...uleb(1), ...uleb(2)])]),
    );
    expect(() => parseImports(module)).toThrow(/Unsupported limits flags/);
  });

  it('rejects a maximum below the initial size', () => {
    const module = wasm(importSection(HOST_MEMORY(4, 2)));
    expect(() => parseImports(module)).toThrow(/below initial/);
  });

  it('rejects duplicate import sections', () => {
    const module = wasm(importSection(HOST_MEMORY(1, 2)), importSection(HOST_MEMORY(1, 2)));
    expect(() => parseImports(module)).toThrow(/Duplicate import section/);
  });
});

// --- readSharedMemoryLimits -------------------------------------------------------

describe('readSharedMemoryLimits', () => {
  it('returns the host.memory limits ready for a shared Memory', () => {
    expect(readSharedMemoryLimits(moduleWithHostMemory(2, 32))).toEqual({ initial: 2, maximum: 32 });
  });

  it('rejects a module without host.memory', () => {
    expect(() => readSharedMemoryLimits(wasm())).toThrow(/does not import host\.memory/);
    const otherMemory = wasm(
      importSection([...name('env'), ...name('memory'), ...MEMORY(0b11, [...uleb(1), ...uleb(2)])]),
    );
    expect(() => readSharedMemoryLimits(otherMemory)).toThrow(/does not import host\.memory/);
  });

  it('rejects a function import named host.memory', () => {
    const module = wasm(importSection([...name('host'), ...name('memory'), ...FUNC(0)]));
    expect(() => readSharedMemoryLimits(module)).toThrow(/does not import host\.memory/);
  });

  it('rejects multiple host.memory imports', () => {
    const module = wasm(importSection(HOST_MEMORY(1, 2), HOST_MEMORY(1, 2)));
    expect(() => readSharedMemoryLimits(module)).toThrow(/expected exactly one/);
  });

  it('rejects a non-shared host.memory', () => {
    const module = wasm(importSection([...name('host'), ...name('memory'), ...MEMORY(0b01, [...uleb(1), ...uleb(2)])]));
    expect(() => readSharedMemoryLimits(module)).toThrow(/not shared/);
  });

  it('rejects a shared host.memory without a declared maximum', () => {
    const module = wasm(importSection([...name('host'), ...name('memory'), ...MEMORY(0b10, [...uleb(1)])]));
    expect(() => readSharedMemoryLimits(module)).toThrow(/must declare one/);
  });
});
