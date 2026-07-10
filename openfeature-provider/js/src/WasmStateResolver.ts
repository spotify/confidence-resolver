import type { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import type {
  ResolveProcessRequest,
  ResolveProcessResponse,
  RegisterResolveRequest,
} from './proto/confidence/wasm/wasm_api';
import type { ApplyFlagsRequest, ResolvedFlag } from './proto/confidence/flags/resolver/v1/api';
import type { LocalResolver } from './LocalResolver';
import type { ResolveFlagsResponse as SmResolveFlagsResponse } from './proto/state_module/api';
import { StateModule } from './StateModule';

/**
 * Backward-compatibility adapter exposing a compiled state module through the
 * LocalResolver interface. New integrations should use StateModule directly.
 */
export class WasmStateResolver implements LocalResolver {
  private module: StateModule | null = null;

  setResolverState(request: SetResolverStateRequest): void {
    // the state bytes are the compiled state module itself
    const wasmModule = new WebAssembly.Module(
      request.state.buffer.slice(
        request.state.byteOffset,
        request.state.byteOffset + request.state.byteLength,
      ) as ArrayBuffer,
    );
    this.module = new StateModule(wasmModule);
  }

  resolveProcess(request: ResolveProcessRequest): ResolveProcessResponse {
    const module = this.module;
    if (!module) throw new Error('resolver state not set');

    if (request.withoutMaterializations) {
      return this.toProcessResponse(module.resolveFlags(request.withoutMaterializations));
    }

    if (request.staticMaterializations) {
      const { resolveRequest, materializations } = request.staticMaterializations;
      return this.toProcessResponse(module.resolveFlags(resolveRequest!, materializations));
    }

    if (request.deferredMaterializations) {
      const smResponse = module.resolveProcessStart(request.deferredMaterializations);
      if (smResponse.resolved) {
        return this.toProcessResponse(smResponse.resolved);
      }
      const { processId, materializationToRead } = smResponse.suspended!;
      return {
        suspended: {
          materializationsToRead: materializationToRead,
          state: this.encodeProcessId(processId),
        },
      };
    }

    if (request.resume) {
      const processId = this.decodeProcessId(request.resume.state);
      return this.toProcessResponse(module.resolveProcessResume(processId, request.resume.materializations));
    }

    throw new Error('empty ResolveProcessRequest');
  }

  // the state_module response is wire-compatible with the v1 response, minus
  // resolveId/flagSchema which a decode of the same bytes would default anyway
  private toProcessResponse(sm: SmResolveFlagsResponse): ResolveProcessResponse {
    return {
      resolved: {
        response: {
          resolvedFlags: sm.resolvedFlags as ResolvedFlag[],
          resolveToken: sm.resolveToken,
          resolveId: '',
        },
        materializationsToWrite: sm.materializationToWrite,
      },
    };
  }

  // suspended process_id is tunneled through the opaque `state` bytes
  private encodeProcessId(id: number): Uint8Array {
    const buf = new Uint8Array(4);
    new DataView(buf.buffer).setUint32(0, id, true);
    return buf;
  }

  private decodeProcessId(state: Uint8Array): number {
    return new DataView(state.buffer, state.byteOffset, state.byteLength).getUint32(0, true);
  }

  registerResolve(_request: RegisterResolveRequest): void {
    // no-op: state module has no equivalent export
  }

  flushLogs(): Uint8Array {
    // logs are pushed via the module's write_logs import, not pulled
    this.module?.flushLogs();
    return new Uint8Array(0);
  }

  flushAssigned(): Uint8Array {
    // no-op: assignment events go through write_logs
    return new Uint8Array(0);
  }

  applyFlags(_request: ApplyFlagsRequest): void {
    // StateModule.applyFlags exists, but mapping the v1 request (google
    // timestamps) onto state_module.ApplyRequest is not wired up yet
    throw new Error('not implemented');
  }

  prometheusSnapshot(_instance: string): string {
    // no-op: state module has no prometheus export
    return '';
  }
}
