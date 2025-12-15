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
  export type WriteOp = WriteOp.Variant | never;
}
export interface MaterializationStore {
  readMaterializations(readOps: MaterializationStore.ReadOp[]): Promise<MaterializationStore.ReadResult[]>;

  writeMaterializations?(writeOps: MaterializationStore.WriteOp[]): Promise<void>;
}
