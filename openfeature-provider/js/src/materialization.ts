import { Fetch } from './fetch';
import {
  ReadOp as ProtoReadOp,
  ReadOperationsRequest,
  ReadOperationsResult,
  ReadResult,
  VariantData,
  WriteOperationsRequest,
} from './proto/confidence/flags/resolver/v1/internal_api';

export namespace MaterializationStore {
  export namespace ReadOp {
    export interface Variant {
      readonly op: 'variant';
      readonly unit: string;
      readonly materialization: string;
      readonly rule: string;
    }
    export interface Inclusion {
      readonly op: 'inclusion';
      readonly unit: string;
      readonly materialization: string;
    }
  }
  export type ReadOp = ReadOp.Variant | ReadOp.Inclusion;

  export namespace ReadResult {
    export interface Variant {
      readonly op: 'variant';
      readonly unit: string;
      readonly materialization: string;
      readonly rule: string;
      readonly variant?: string;
    }
    export interface Inclusion {
      readonly op: 'inclusion';
      readonly unit: string;
      readonly materialization: string;
      readonly included: boolean;
    }
  }
  export type ReadResult = ReadResult.Inclusion | ReadResult.Variant;

  export namespace WriteOp {
    export interface Variant {
      readonly op: 'variant';
      readonly unit: string;
      readonly materialization: string;
      readonly rule: string;
      readonly variant: string;
    }
  }
  export type WriteOp = WriteOp.Variant;
}
/**
 * Interface for storing and retrieving materialization data.
 *
 * Implementations can use any storage backend (e.g., Redis, BigTable, Cassandra, DynamoDB,
 * or simple key-value stores) as long as they fulfill this contract.
 *
 * @see The README for conceptual documentation on sticky assignments and materialized segments.
 */
export interface MaterializationStore {
  /**
   * Reads materialization data for the given operations.
   *
   * For `variant` operations, returns the stored variant assignment (if any) for a unit/materialization/rule combination.
   * For `inclusion` operations, returns whether a unit is included in a materialized segment.
   *
   * @param readOps - The read operations to perform.
   * @returns Results for each operation. The order of results does not need to match the order of operations;
   *          callers match results to operations using the `unit`, `materialization`, and `op` fields.
   */
  readMaterializations(readOps: MaterializationStore.ReadOp[]): Promise<MaterializationStore.ReadResult[]>;

  /**
   * Persists variant assignments for sticky bucketing.
   *
   * This method is optional. Omit it for read-only stores that only serve pre-populated
   * materialized segments without supporting runtime sticky assignment writes.
   *
   * @param writeOps - The variant assignments to persist.
   */
  writeMaterializations?(writeOps: MaterializationStore.WriteOp[]): Promise<void>;
}

export class ConfidenceRemoteMaterializationStore implements MaterializationStore {
  constructor(
    private flagClientSecret: string,
    private fetch: Fetch = globalThis.fetch,
    private signal?: AbortSignal,
  ) {}

  async readMaterializations(readOps: MaterializationStore.ReadOp[]): Promise<MaterializationStore.ReadResult[]> {
    const response = await this.fetch('https://resolver.confidence.dev/v1/materialization:readMaterializedOperations', {
      method: 'post',
      signal: this.signal,
      headers: {
        'Content-Type': 'application/x-protobuf',
        Authorization: `ClientSecret ${this.flagClientSecret}`,
      },
      body: ReadOperationsRequest.encode(readOpsToProto(readOps)).finish(),
    });
    if (!response.ok) {
      throw new Error(`Failed to read materializations: ${response.status} ${response.statusText}`);
    }
    return readResultFromProto(ReadOperationsResult.decode(new Uint8Array(await response.arrayBuffer())));
  }

  async writeMaterializations(writeOps: MaterializationStore.WriteOp[]): Promise<void> {
    const response = await this.fetch(
      'https://resolver.confidence.dev/v1/materialization:writeMaterializedOperations',
      {
        method: 'post',
        signal: this.signal,
        headers: {
          'Content-Type': 'application/x-protobuf',
          Authorization: `ClientSecret ${this.flagClientSecret}`,
        },
        body: WriteOperationsRequest.encode(writeOpsToProto(writeOps)).finish(),
      },
    );
    if (!response.ok) {
      throw new Error(`Failed to write materializations: ${response.status} ${response.statusText}`);
    }
  }
}

export function readOpsToProto(readOps: MaterializationStore.ReadOp[]): ReadOperationsRequest {
  return {
    ops: readOps.flatMap((readOp): ProtoReadOp[] => {
      switch (readOp.op) {
        case 'inclusion':
          return [
            ProtoReadOp.create({
              inclusionReadOp: {
                unit: readOp.unit,
                materialization: readOp.materialization,
              },
            }),
          ];
        case 'variant':
          return [
            ProtoReadOp.create({
              variantReadOp: {
                unit: readOp.unit,
                materialization: readOp.materialization,
                rule: readOp.rule,
              },
            }),
          ];
      }
      return [];
    }),
  };
}

export function readOpsFromProto(readOpReq: ReadOperationsRequest): MaterializationStore.ReadOp[] {
  return readOpReq.ops.flatMap(({ variantReadOp, inclusionReadOp }): MaterializationStore.ReadOp[] => {
    if (variantReadOp) {
      return [
        {
          op: 'variant',
          unit: variantReadOp.unit,
          materialization: variantReadOp.materialization,
          rule: variantReadOp.rule,
        },
      ];
    }
    if (inclusionReadOp) {
      return [
        {
          op: 'inclusion',
          unit: inclusionReadOp.unit,
          materialization: inclusionReadOp.materialization,
        },
      ];
    }
    return [];
  });
}

export function readResultFromProto(result: ReadOperationsResult): MaterializationStore.ReadResult[] {
  return result.results.flatMap(({ inclusionResult, variantResult }): MaterializationStore.ReadResult[] => {
    if (inclusionResult) {
      return [
        {
          op: 'inclusion',
          unit: inclusionResult.unit,
          materialization: inclusionResult.materialization,
          included: inclusionResult.isIncluded,
        },
      ];
    }
    if (variantResult) {
      return [
        {
          op: 'variant',
          unit: variantResult.unit,
          materialization: variantResult.materialization,
          rule: variantResult.rule,
          variant: variantResult.variant,
        },
      ];
    }
    return [];
  });
}

export function readResultToProto(readResults: MaterializationStore.ReadResult[]): ReadOperationsResult {
  return {
    results: readResults.flatMap((readResult): ReadResult[] => {
      switch (readResult.op) {
        case 'inclusion':
          return [
            {
              inclusionResult: {
                unit: readResult.unit,
                materialization: readResult.materialization,
                isIncluded: readResult.included,
              },
            },
          ];
        case 'variant':
          return [
            {
              variantResult: {
                unit: readResult.unit,
                materialization: readResult.materialization,
                rule: readResult.rule,
                variant: readResult.variant ?? '',
              },
            },
          ];
      }
      return [];
    }),
  };
}

function writeOpsToProto(writeOps: MaterializationStore.WriteOp[]): WriteOperationsRequest {
  return {
    storeVariantOp: writeOps.flatMap((writeOp): VariantData[] => {
      switch (writeOp.op) {
        case 'variant':
          return [
            {
              unit: writeOp.unit,
              materialization: writeOp.materialization,
              rule: writeOp.rule,
              variant: writeOp.variant,
            },
          ];
      }
      return [];
    }),
  };
}

export function writeOpsFromProto(writeOpsReq: WriteOperationsRequest): MaterializationStore.WriteOp[] {
  return writeOpsReq.storeVariantOp.map((variantData): MaterializationStore.WriteOp => {
    return {
      op: 'variant',
      unit: variantData.unit,
      materialization: variantData.materialization,
      rule: variantData.rule,
      variant: variantData.variant,
    };
  });
}
