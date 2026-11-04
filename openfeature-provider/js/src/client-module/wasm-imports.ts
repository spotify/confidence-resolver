/**
 * Raw-byte parsing of a compiled client module's import section.
 *
 * The compiled-module contract requires the host to create ONE shared memory
 * sized exactly from the module's `host.memory` import limits (minimum and
 * maximum). JavaScript cannot read an imported memory's declared limits
 * through `WebAssembly.Module.imports()`, so the import section is parsed
 * directly from the module bytes. This also allows validating an artifact
 * before spending time compiling it.
 *
 * Works on raw bytes only: nothing here instantiates or compiles the module.
 */

/** A memory import's declared limits, in pages. */
export interface MemoryLimits {
  /** Declared minimum, in pages. */
  initial: number;
  /**
   * Declared maximum, in pages. `undefined` when the import declares none.
   * A shared memory must declare one.
   */
  maximum?: number;
  /** Whether the limits are marked shared (WASM threads proposal). */
  shared: boolean;
}

/** One entry of the wasm import section. */
export interface WasmImport {
  module: string;
  field: string;
  kind: 'function' | 'table' | 'memory' | 'global';
  /** Present when `kind` is `'memory'`. */
  memory?: MemoryLimits;
}

/** Limits of the `host.memory` import, ready for `new WebAssembly.Memory(...)`. */
export interface SharedMemoryDescriptor {
  initial: number;
  maximum: number;
}

const IMPORT_SECTION_ID = 2;
const WASM_HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

class ByteReader {
  private offset: number;
  private end: number;

  constructor(
    private readonly wasm: Uint8Array,
    startOffset = 0,
  ) {
    this.offset = startOffset;
    this.end = wasm.length;
  }

  get position(): number {
    return this.offset;
  }

  /** Narrow the readable range to the next `size` bytes; returns the previous end. */
  enter(size: number, what: string): number {
    const previousEnd = this.end;
    const end = this.offset + size;
    if (end > this.wasm.length) {
      throw new Error(
        `${what} extends past the end of the wasm binary: needs ${size} bytes at ${this.offset}, only ${this.wasm.length - this.offset} remain`,
      );
    }
    this.end = end;
    return previousEnd;
  }

  /** Restore the range after `enter`; the cursor must sit exactly at the narrowed end. */
  leave(previousEnd: number, what: string): void {
    if (this.offset !== this.end) {
      throw new Error(`Malformed ${what}: ${this.end - this.offset} trailing byte(s)`);
    }
    this.end = previousEnd;
  }

  byte(what: string): number {
    if (this.offset >= this.end) {
      throw new Error(
        `Truncated wasm binary: reading ${what} at byte ${this.offset} but the section ends at ${this.end}`,
      );
    }
    return this.wasm[this.offset++];
  }

  uleb(what: string): number {
    let result = 0;
    let shift = 0;
    for (let i = 0; ; i++) {
      if (i >= 5) {
        throw new Error(`Malformed uleb128 for ${what}: runs past 32 bits`);
      }
      const byte = this.byte(what);
      const bits = byte & 0x7f;
      if (i === 4 && bits > 0x0f) {
        throw new Error(`Malformed uleb128 for ${what}: exceeds 32 bits`);
      }
      result += bits * 2 ** shift;
      shift += 7;
      if ((byte & 0x80) === 0) {
        return result >>> 0;
      }
    }
  }

  name(what: string): string {
    const length = this.uleb(`${what} length`);
    if (this.offset + length > this.end) {
      throw new Error(
        `Truncated wasm binary: ${what} needs ${length} bytes at ${this.offset} but the section ends at ${this.end}`,
      );
    }
    const bytes = this.wasm.subarray(this.offset, this.offset + length);
    this.offset += length;
    return new TextDecoder().decode(bytes);
  }

  skip(size: number, what: string): void {
    if (this.offset + size > this.end) {
      throw new Error(`Truncated wasm binary: skipping ${what} needs ${size} bytes at ${this.offset}`);
    }
    this.offset += size;
  }
}

function readLimits(reader: ByteReader, what: string): MemoryLimits {
  const flags = reader.byte(`${what} limits flags`);
  const hasMaximum = (flags & 0x01) !== 0;
  const shared = (flags & 0x02) !== 0;
  if (flags & ~0x03) {
    throw new Error(`Unsupported limits flags 0x${flags.toString(16)} for ${what}`);
  }
  const initial = reader.uleb(`${what} initial pages`);
  if (!hasMaximum) {
    return { initial, shared };
  }
  const maximum = reader.uleb(`${what} maximum pages`);
  if (maximum < initial) {
    throw new Error(`${what} declares maximum ${maximum} below initial ${initial}`);
  }
  return { initial, maximum, shared };
}

function verifyHeader(wasm: Uint8Array): void {
  if (wasm.length < WASM_HEADER.length) {
    throw new Error(`Not a wasm binary: ${wasm.length} byte(s) is shorter than the 8-byte header`);
  }
  for (let i = 0; i < WASM_HEADER.length; i++) {
    if (wasm[i] !== WASM_HEADER[i]) {
      throw new Error(i < 4 ? 'Not a wasm binary: bad magic' : `Unsupported wasm version at byte ${i}`);
    }
  }
}

/**
 * Parse every import of the module from its raw bytes, without compiling it.
 *
 * Returns an empty list for a module without an import section.
 * Throws on malformed binaries (truncated sections, trailing bytes, invalid
 * uleb128 values, unsupported limit flags).
 */
export function parseImports(wasm: Uint8Array): WasmImport[] {
  verifyHeader(wasm);
  const reader = new ByteReader(wasm, WASM_HEADER.length);
  const imports: WasmImport[] = [];
  let sawImportSection = false;

  while (reader.position < wasm.length) {
    const sectionId = reader.byte('section id');
    const size = reader.uleb('section size');
    if (sectionId !== IMPORT_SECTION_ID) {
      reader.skip(size, `section id ${sectionId}`);
      continue;
    }
    if (sawImportSection) {
      throw new Error('Duplicate import section');
    }
    sawImportSection = true;

    const previousEnd = reader.enter(size, 'import section');
    const count = reader.uleb('import count');
    for (let i = 0; i < count; i++) {
      const module = reader.name('import module name');
      const field = reader.name('import field name');
      const what = `import ${module}.${field}`;
      const kindByte = reader.byte(`import kind for ${what}`);
      switch (kindByte) {
        case 0x00:
          reader.uleb(`${what} type index`);
          imports.push({ module, field, kind: 'function' });
          break;
        case 0x01: {
          reader.byte(`${what} element type`);
          readLimits(reader, `${what} table`);
          imports.push({ module, field, kind: 'table' });
          break;
        }
        case 0x02: {
          const memory = readLimits(reader, `${what} memory`);
          imports.push({ module, field, kind: 'memory', memory });
          break;
        }
        case 0x03:
          reader.byte(`${what} global value type`);
          reader.byte(`${what} global mutability`);
          imports.push({ module, field, kind: 'global' });
          break;
        default:
          throw new Error(`Unknown import kind 0x${kindByte.toString(16)} for ${what}`);
      }
    }
    reader.leave(previousEnd, 'import section');
  }
  return imports;
}

/**
 * Read the `host.memory` import limits of a compiled client module.
 *
 * The contract requires exactly one shared `host.memory` import with a
 * declared maximum; the returned descriptor is ready for
 * `new WebAssembly.Memory({ ...descriptor, shared: true })`. Throws when the
 * module does not match the contract.
 */
export function readSharedMemoryLimits(wasm: Uint8Array): SharedMemoryDescriptor {
  const hostMemories = parseImports(wasm).filter(
    imp => imp.kind === 'memory' && imp.module === 'host' && imp.field === 'memory',
  );
  if (hostMemories.length === 0) {
    throw new Error('Module does not import host.memory');
  }
  if (hostMemories.length > 1) {
    throw new Error(`Module imports host.memory ${hostMemories.length} times, expected exactly one`);
  }
  const { initial, maximum, shared } = hostMemories[0].memory!;
  if (!shared) {
    throw new Error('host.memory import is not shared: the contract requires shared memory (WASM threads)');
  }
  if (maximum === undefined) {
    throw new Error('host.memory import declares no maximum: shared memories must declare one');
  }
  return { initial, maximum };
}
