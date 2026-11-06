;; Bare guest: the required export set with trivial bodies and NO optional
;; exports (metrics_snapshot / observe_client_resolve_latency / evaluate are
;; absent by design). Used to test the host's optional-export capability
;; detection. The import set matches the contract exactly.
;;
;; Regenerate the committed .wasm with: wat2wasm --enable-threads bare-guest.wat -o bare-guest.wasm

(module
  (import "host" "memory" (memory 1 16 shared))
  (import "env" "current_time" (func $current_time (result i64)))
  (import "env" "random_seed" (func $random_seed (result i64)))
  (import "env" "write_logs" (func $write_logs (param i64)))
  (import "apply_filter" "seen" (func $apply_seen (param i64) (result i32)))
  (import "schema_filter" "seen" (func $schema_seen (param i64) (result i32)))

  (func $init (drop (call $random_seed)))
  (start $init)

  (func (export "alloc") (param $size i32) (result i32) (i32.const 0))
  (func (export "free") (param $ptr i32))
  (func (export "resolve_process_start") (param $request i64) (result i64) (i64.const 0))
  (func (export "resolve_process_resume") (param $process i32) (param $materializations i64) (result i64) (i64.const 0))
  (func (export "resolve_process_discard") (param $process i32))
  (func (export "apply_flags") (param $request i64) (result i32) (i32.const 0))
  (func (export "flush_logs"))
  (func (export "ack_logs") (param $payload i64))
  (func (export "allocated_bytes") (result i32) (i32.const 0))
  (func (export "destroy_instance"))
)
