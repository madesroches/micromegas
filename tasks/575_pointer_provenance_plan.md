# Pointer Provenance Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/575

## Overview
In-process events store `&'static` references as raw integers in the block queue and rebuild
references on read. `TaggedLogString::read_value` does so with `transmute::<u64, &T>`, which the
compiler flags (`integer_to_ptr_transmutes`) and which is silenced with an `allow`. Replace the
integer<->pointer conversions with the exposed-provenance API so the code is sound under Rust's
pointer provenance model, and remove the `allow`.

## Current State
The issue names `rust/tracing/src/logs/events.rs`; the code now lives in
`rust/tracing/src/logs/log_events.rs`.

- `TaggedLogString` (`log_events.rs:261`)
  - `write_value` (l.292) does `write_any(buffer, &self.desc)`, copying the bytes of the
    reference without exposing its provenance.
  - `read_value` (l.303-317) reads a `u64` and `transmute`s it to `&LogMetadata<'static>`, under
    `#[allow(unknown_lints)]` / `#[allow(integer_to_ptr_transmutes)]` with a TODO pointing to
    the backlog doc.
- `LogStringEvent` (`log_events.rs:57-83`) has the same round trip using `as usize` /
  `as *const LogMetadata` casts (no transmute, no allow, but the same provenance gap).
- `PropertySetId` (`property_set.rs:203-221`) stores `set as *const PropertySet as u64` and
  rebuilds with `self.0 as *const PropertySet`. It is written verbatim by `TaggedLogString` and
  `TaggedLogInteropEvent`.
- The 64-bit guard `const _: () = assert!(size_of::<usize>() == 8)` (`log_events.rs:13`) stays.
- `StaticStringRef` (`static_string_ref.rs`) holds `ptr: u64` built with `as_ptr() as u64` and
  rebuilt with `as *const u8`. Same class, but it is a `#[repr(C)]` wire struct shared with
  Unreal and used as a dependency id, so it is left out of this change.

## Design
Use the stable (Rust 1.84+) strict-provenance APIs, which keep the u64 wire format unchanged:

- Write: `ptr.expose_provenance() as u64` (pointer -> address, marking provenance exposed).
- Read: `&*std::ptr::with_exposed_provenance::<T>(addr as usize)`.

Add two small helpers next to the `InProcSerialize` impls in `log_events.rs` so the
conversion lives in one place (DRY) instead of being repeated per event:

- `fn ref_to_addr<T>(r: &'static T) -> u64`
- `unsafe fn addr_to_ref<T>(addr: u64) -> &'static T`

Apply them to:
1. `TaggedLogString::write_value` / `read_value` (the reported site); delete both `allow`s.
2. `LogStringEvent::write_value` / `read_value`.
3. `PropertySetId::interned` / `as_interned` (`property_set.rs`), keeping the bit-0 check.

`LogMetadata<'static>` pointers are only ever produced from `&'static` values, so exposing at
write time and re-deriving at read time is valid; the on-wire bytes are identical, so no
`SCHEMA_VERSION` or analytics-side change.

Confirm the workspace MSRV is at least 1.84 before using the APIs.

## Implementation Steps
1. Check `rust-version` / toolchain supports `expose_provenance` / `with_exposed_provenance`.
2. Add the helpers in `rust/tracing/src/logs/log_events.rs`.
3. Switch `TaggedLogString` read/write to them; remove the `allow` attributes and TODO.
4. Switch `LogStringEvent` read/write.
5. Switch `PropertySetId::interned` / `as_interned` in `rust/tracing/src/property_set.rs`.
6. Add a `CHANGELOG.md` entry (internal soundness fix, no behavior change).
7. Move this plan to `tasks/completed/` when done.

## Files to Modify
- `rust/tracing/src/logs/log_events.rs`
- `rust/tracing/src/property_set.rs`
- `CHANGELOG.md`

## Trade-offs
- `with_exposed_provenance` vs strict `ptr::with_addr`: strict provenance can't work here
  because the pointer is rebuilt from an address stored in a byte buffer with no pointer to
  derive from, so the exposed-provenance model is the right fit.
- Storing a real `*const T` in the buffer instead of `u64`: rejected, it would change the wire
  layout shared with the analytics parser.
- `StaticStringRef` is not touched here (see Current State); widening scope there means
  changing a cross-language struct for no observable gain.

## Testing Strategy
- Existing round-trip tests in `rust/tracing/tests` (`stream_flush_tests.rs`,
  `property_macro_tests.rs`, `dyn_deps_tests.rs`) and `rust/analytics/tests/log_tests.rs`
  already serialize and parse `TaggedLogString` / `LogStringEvent`; they must pass unchanged.
- Add a no-DB unit test in `log_events.rs` that writes a `TaggedLogString` pointing at a
  `static` `LogMetadata` and an interned `PropertySet`, reads it back, and asserts the
  references are pointer-equal to the originals.
- `cargo clippy --workspace -- -D warnings` must pass with the `allow` removed.

## Manual Verification
Run `cargo +nightly miri test -p micromegas-tracing` on the new unit test if Miri is
installed. Not automated in CI because Miri needs a nightly toolchain and is slow; expected
result: no provenance errors.
