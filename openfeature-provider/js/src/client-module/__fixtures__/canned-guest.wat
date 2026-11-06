;; Canned compiled client-module guest for testing the module RUNTIME.
;;
;; Like fake-guest, this implements the contract's import/export surface, but
;; resolve responses are staged BY THE HOST: the host writes serialized
;; ResolveProcessResponse / ResolutionDetails bytes into fixed regions of the
;; shared memory before calling, and the guest copies them out as module-owned
;; response slices. resolve_process_start consumes its request, discards are
;; counted, flush_logs delivers a real WriteLogsPayload {parts:[slice]} whose
;; concatenated body is "LOGSBODYLOGSBODY".
;;
;; Shared memory layout (little-endian):
;;   [0..4)    i32 bump-allocator top (atomic); arena starts at 48
;;   [4..8)    i32 destroy_instance counter (atomic)
;;   [8..16)   i64 last ack_logs payload
;;   [16..24)  i64 last write_logs payload
;;   [24..32)  i64 last random_seed value
;;   [32..36)  i32 length of the staged start response; byte [35] doubles as
;;             the embed-log_token flag (top byte — lengths stay < 16MiB)
;;   [36..40)  i32 length of the staged resume response
;;   [40..44)  i32 length of the staged evaluate details
;;   [44..48)  i32 resolve_process_discard counter
;;   [48..)    allocation arena
;;   0xE000..  staged start response (max 1KiB)
;;   0xE400..  staged resume response (max 1KiB)
;;   0xE800..  staged evaluate details (max 1KiB)
;;
;; Regenerate the committed .wasm with: wat2wasm --enable-threads canned-guest.wat -o canned-guest.wasm

(module
  (import "host" "memory" (memory 1 16 shared))
  (import "env" "current_time" (func $current_time (result i64)))
  (import "env" "random_seed" (func $random_seed (result i64)))
  (import "env" "write_logs" (func $write_logs (param i64)))
  (import "apply_filter" "seen" (func $apply_seen (param i64) (result i32)))
  (import "schema_filter" "seen" (func $schema_seen (param i64) (result i32)))

  (func $init
    (i64.atomic.store (i32.const 24) (call $random_seed))
    (if (i32.eq (i32.atomic.load (i32.const 0)) (i32.const 0))
      (then (i32.atomic.store (i32.const 0) (i32.const 48)))))
  (start $init)

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

  ;; Copy the staged region [addr, addr+len) as a fresh module-owned slice.
  (func $canned (param $addr i32) (param $len i32) (result i64)
    (local $dst i32)
    (if (i32.eqz (local.get $len))
      (then (return (i64.const 0))))
    (local.set $dst (call $alloc (local.get $len)))
    (if (i32.eqz (local.get $dst))
      (then (unreachable)))
    (call $copy (local.get $dst) (local.get $addr) (local.get $len))
    (call $pack (local.get $dst) (local.get $len)))

  (func (export "resolve_process_start") (param $request i64) (result i64)
    (call $free (call $unpack_ptr (local.get $request)))       ;; consume the request
    (call $canned (i32.const 0xE000) (i32.load (i32.const 32))))

  (func (export "resolve_process_resume") (param $process i32) (param $materializations i64) (result i64)
    (call $free (call $unpack_ptr (local.get $materializations)))
    (call $canned (i32.const 0xE400) (i32.load (i32.const 36))))

  (func (export "resolve_process_discard") (param $process i32)
    (drop (i32.atomic.rmw.add (i32.const 44) (i32.const 1))))

  (func (export "apply_flags") (param $request i64) (result i32)
    ;; consult the exposure dedup filter on the first 8 request bytes (the
    ;; client-secret prefix: stable across calls with the same credential)
    (if (call $apply_seen (i64.load (call $unpack_ptr (local.get $request))))
      (then (return (i32.const 1))))
    (call $free (call $unpack_ptr (local.get $request)))
    (i32.const 0))

  ;; flush_logs: body = "LOGSBODYLOGSBODY" (16B), payload = a serialized
  ;; WriteLogsPayload {parts: [packed(body)]}: field 1 packed fixed64 →
  ;; [0x0A, 0x08, <packed slice, little-endian>]. When byte [35] is staged
  ;; non-zero, also appends field 2 log_token "moduletok" — exercising the
  ;; host's payload-token preference over the wrapper token.
  (func (export "flush_logs")
    (local $body i32) (local $payload i32) (local $slice i64) (local $size i32)
    (local.set $body (call $alloc (i32.const 16)))
    (if (i32.eqz (local.get $body))
      (then (unreachable)))
    (i64.store (local.get $body) (i64.const 0x59444F4253474F4C))              ;; "LOGSBODY"
    (i64.store (i32.add (local.get $body) (i32.const 8)) (i64.const 0x59444F4253474F4C))
    (local.set $slice (call $pack (local.get $body) (i32.const 16)))
    (local.set $payload (call $alloc (i32.const 21)))
    (if (i32.eqz (local.get $payload))
      (then (unreachable)))
    (i32.store8 (local.get $payload) (i32.const 0x0A))
    (i32.store8 (i32.add (local.get $payload) (i32.const 1)) (i32.const 0x08))
    (i64.store (i32.add (local.get $payload) (i32.const 2)) (local.get $slice))
    (local.set $size (i32.const 10))
    (if (i32.load8_u (i32.const 35))
      (then
        (i32.store8 (i32.add (local.get $payload) (i32.const 10)) (i32.const 0x12))   ;; field 2 tag
        (i32.store8 (i32.add (local.get $payload) (i32.const 11)) (i32.const 0x09))    ;; len 9
        (i64.store (i32.add (local.get $payload) (i32.const 12)) (i64.const 0x6F74656C75646F6D)) ;; "moduleto"
        (i32.store8 (i32.add (local.get $payload) (i32.const 20)) (i32.const 0x6B))    ;; "k"
        (local.set $size (i32.const 21))))
    (i64.store (i32.const 16) (call $pack (local.get $payload) (local.get $size)))
    (call $write_logs (call $pack (local.get $payload) (local.get $size))))

  (func (export "ack_logs") (param $payload i64)
    (i64.store (i32.const 8) (local.get $payload)))
  (func (export "allocated_bytes") (result i32)
    (i32.sub (i32.atomic.load (i32.const 0)) (i32.const 48)))
  (func (export "metrics_snapshot") (param $format i32) (result i64)
    (local $dst i32)
    (local.set $dst (call $alloc (i32.const 12)))
    (if (i32.eqz (local.get $dst))
      (then (unreachable)))
    (i64.store (local.get $dst) (i64.const 0x74656D2D656B6166))              ;; "fake-me"
    (i32.store (i32.add (local.get $dst) (i32.const 8)) (i32.const 0x73636972)) ;; "trics"
    (call $pack (local.get $dst) (i32.const 12)))
  (func (export "observe_client_resolve_latency") (param $client i32) (param $micros i64))
  (func (export "evaluate") (param $response i64) (param $request i64) (result i64)
    (call $free (call $unpack_ptr (local.get $request)))                      ;; consume the request
    (call $canned (i32.const 0xE800) (i32.load (i32.const 40))))
  (func (export "destroy_instance")
    (drop (i32.atomic.rmw.add (i32.const 4) (i32.const 1))))
)
