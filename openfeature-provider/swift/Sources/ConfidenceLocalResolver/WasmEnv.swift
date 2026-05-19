import Foundation
import WasmKit

/// Holds references to the WASM exports that the host needs to call (memory, alloc, free).
///
/// Created empty before instantiation so that host-import closures can capture it by
/// reference, then populated immediately after `Module.instantiate` returns.
final class WasmEnv {
    var memory: Memory?
    var alloc: Function?
    var free: Function?
}
