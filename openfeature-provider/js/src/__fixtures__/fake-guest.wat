;; Fake compiled client-module guest for testing the host ABI layer.
;;
;; NOT the real compiled module: it implements the contract's export/import
;; surface with echo/trivial behaviour so the host layer (GuestInstance,
;; GuestInstancePool) can be tested without real artifacts. Behaviours that
;; matter to the host are faithful to the contract:
;;   - host.memory is an IMPORTED SHARED memory; all state lives in it and is
;;     shared across every instance (bump arena, counters).
;;   - alloc/free follow the Slice ownership rules (requests are consumed,
;;     responses are module-owned until the host frees them).
;;   - flush_logs delivers a 16-byte payload through env.write_logs; the host
;;     must ack_logs it.
;;   - apply_flags consults apply_filter.seen on the request length: returns 1
;;     when the fingerprint was already seen, 0 otherwise.
;;
;; Shared memory layout (little-endian):
;;   [0..4)   i32  bump-allocator top (atomic); arena starts at 32
;;   [4..8)   i32  destroy_instance counter (atomic)
;;   [8..16)  i64  last ack_logs payload
;;   [16..24) i64  last write_logs payload
;;   [24..32) i64  last random_seed value
;;   [32..)        allocation arena
;;
;; Regenerate the committed .wasm with: wat2wasm --enable-threads fake-guest.wat -o fake-guest.wasm

(module
  (import "host" "memory" (memory 1 16 shared))
  (import "env" "current_time" (func $current_time (result i64)))
  (import "env" "random_seed" (func $random_seed (result i64)))
  (import "env" "write_logs" (func $write_logs (param i64)))
  (import "apply_filter" "seen" (func $apply_seen (param i64) (result i32)))
  (import "schema_filter" "seen" (func $schema_seen (param i64) (result i32)))

  (func $init
    ;; random_seed is called once per instance (the guide's contract)
    (i64.atomic.store (i32.const 24) (call $random_seed))
    ;; the arena is initialised only once per memory, not per instance
    (if (i32.eq (i32.atomic.load (i32.const 0)) (i32.const 0))
      (then (i32.atomic.store (i32.const 0) (i32.const 32)))))
  (start $init)

  ;; alloc: 8-aligned bump allocation with a CAS loop (instances share the arena)
  (func $alloc (export "alloc") (param $size i32) (result i32)
    (local $top i32) (local $aligned i32) (local $newTop i32) (local $limit i32)
    (local.set $limit (i32.mul (memory.size) (i32.const 65536)))
    (loop $retry
      (local.set $top (i32.atomic.load (i32.const 0)))
      (local.set $aligned (i32.and (i32.add (local.get $top) (i32.const 7)) (i32.const -8)))
      (local.set $newTop (i32.add (local.get $aligned) (local.get $size)))
      (if (i32.gt_u (local.get $newTop) (local.get $limit))
        (then (return (i32.const 0))))
      (br_if $retry
        (i32.ne
          (i32.atomic.rmw.cmpxchg (i32.const 0) (local.get $top) (local.get $newTop))
          (local.get $top))))
    (local.get $aligned))

  ;; bump allocator: free is a no-op, as if every block lived until rotation
  (func $free (export "free") (param $ptr i32))

  (func $pack (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.extend_i32_u (local.get $ptr))
      (i64.shl (i64.extend_i32_u (local.get $len)) (i64.const 32))))
  (func $unpack_ptr (param $slice i64) (result i32) (i32.wrap_i64 (local.get $slice)))
  (func $unpack_len (param $slice i64) (result i32)
    (i32.wrap_i64 (i64.shr_u (local.get $slice) (i64.const 32))))

  (func $copy (param $dst i32) (param $src i32) (param $len i32)
    (local $i i32)
    (loop $copy_loop
      (if (i32.lt_u (local.get $i) (local.get $len))
        (then
          (i32.store8 (i32.add (local.get $dst) (local.get $i))
            (i32.load8_u (i32.add (local.get $src) (local.get $i))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $copy_loop)))))

  (func $echo (param $slice i64) (result i64)
    (local $ptr i32) (local $len i32) (local $dst i32)
    (local.set $ptr (call $unpack_ptr (local.get $slice)))
    (local.set $len (call $unpack_len (local.get $slice)))
    (if (i32.eqz (local.get $len))
      (then (return (i64.const 0))))
    (local.set $dst (call $alloc (local.get $len)))
    (if (i32.eqz (local.get $dst))
      (then (unreachable)))
    (call $copy (local.get $dst) (local.get $ptr) (local.get $len))
    (call $free (local.get $ptr))                 ;; requests are consumed
    (call $pack (local.get $dst) (local.get $len)))

  (func (export "resolve_process_start") (param $request i64) (result i64)
    (call $echo (local.get $request)))
  (func (export "resolve_process_resume") (param $process i32) (param $materializations i64) (result i64)
    (call $echo (local.get $materializations)))
  (func (export "resolve_process_discard") (param $process i32))
  (func (export "apply_flags") (param $request i64) (result i32)
    ;; consult the exposure dedup filter on the request length
    (if (call $apply_seen (i64.extend_i32_u (call $unpack_len (local.get $request))))
      (then (return (i32.const 1))))
    (call $free (call $unpack_ptr (local.get $request)))
    (i32.const 0))
  (func (export "flush_logs")
    (local $dst i32) (local $slice i64)
    (local.set $dst (call $alloc (i32.const 16)))
    (if (i32.eqz (local.get $dst))
      (then (unreachable)))
    (i64.store (local.get $dst) (call $current_time))            ;; [0..8): host clock
    (i64.store (i32.add (local.get $dst) (i32.const 8))
      (i64.const 0x53474F4C454B4146))                             ;; [8..16): "FAKELOGS"
    (local.set $slice (call $pack (local.get $dst) (i32.const 16)))
    (i64.store (i32.const 16) (local.get $slice))                 ;; record last write_logs
    (call $write_logs (local.get $slice)))
  (func (export "ack_logs") (param $payload i64)
    (i64.store (i32.const 8) (local.get $payload)))              ;; record last ack
  (func (export "allocated_bytes") (result i32)
    (i32.sub (i32.atomic.load (i32.const 0)) (i32.const 32)))
  (func (export "metrics_snapshot") (param $format i32) (result i64)
    (local $dst i32)
    (local.set $dst (call $alloc (i32.const 12)))
    (if (i32.eqz (local.get $dst))
      (then (unreachable)))
    (i64.store (local.get $dst) (i64.const 0x74656D2D656B6166))   ;; "fake-me"
    (i32.store (i32.add (local.get $dst) (i32.const 8)) (i32.const 0x73636972)) ;; "trics"
    (call $pack (local.get $dst) (i32.const 12)))
  (func (export "observe_client_resolve_latency") (param $client i32) (param $micros i64))
  (func (export "evaluate") (param $response i64) (param $request i64) (result i64)
    ;; the response envelope is borrowed: never freed here
    (call $echo (local.get $request)))
  (func (export "destroy_instance")
    (drop (i32.atomic.rmw.add (i32.const 4) (i32.const 1))))
  ;; test-only passthroughs so the filter imports are exercised through a call
  (func (export "check_apply_filter") (param $fingerprint i64) (result i32)
    (call $apply_seen (local.get $fingerprint)))
  (func (export "check_schema_filter") (param $fingerprint i64) (result i32)
    (call $schema_seen (local.get $fingerprint)))
)
