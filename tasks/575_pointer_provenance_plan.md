# Pointer Provenance Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/575

## Overview
In-process events store `&'static` references as raw integers in the block queue and rebuild
references on read. `TaggedLogString::read_value` does so with `transmute::<u64, &T>`, which the
compiler flags (`integer_to_ptr_transmutes`) and which is silenced with an `allow`. Expose the
reference's provenance on write and rebuild it with the exposed-provenance API on read, so the
code is sound under Rust's pointer provenance model, and remove the `allow`.

## Current State
The issue names `rust/tracing/src/logs/events.rs`; the code now lives in
`rust/tracing/src/logs/log_events.rs`.

- `TaggedLogString` (`log_events.rs:261`)
  - `write_value` (l.292) does `write_any(buffer, &self.desc)`, copying the bytes of the
    reference without exposing its provenance.
  - `read_value` (l.303-317) reads a `u64` and `transmute`s it to `&LogMetadata<'static>`, under
    `#[allow(unknown_lints)]` / `#[allow(integer_to_ptr_transmutes)]` with a TODO pointing to
    the backlog doc (`tasks/pointer-provenance.md`, which no longer exists).
- The 64-bit guard `const _: () = assert!(size_of::<usize>() == 8)` (`log_events.rs:13`) stays.
- `StaticStringRef` (`static_string_ref.rs`) holds `ptr: u64` built with `as_ptr() as u64` and
  rebuilt with `as *const u8`. Same class, but it is a `#[repr(C)]` wire struct shared with
  Unreal and used as a dependency id, so it is left out of this change.

## Design
Use the stable strict-provenance APIs, which keep the u64 wire format unchanged, inlined in
`TaggedLogString` (the only call site):

- Write: `(self.desc as *const LogMetadata<'static>).expose_provenance() as u64`, written to the
  buffer instead of the raw reference bytes.
- Read: `&*std::ptr::with_exposed_provenance::<LogMetadata<'static>>(addr as usize)`, replacing
  the `transmute`.

`LogMetadata<'static>` pointers are only ever produced from `&'static` values, so exposing at
write time and re-deriving at read time is valid; the on-wire bytes are identical, so no
`SCHEMA_VERSION` or analytics-side change.

## Implementation Steps
1. Switch `TaggedLogString::write_value` / `read_value` in `rust/tracing/src/logs/log_events.rs`
   to the calls above; remove the `allow` attributes and the stale TODO.
2. Add a `CHANGELOG.md` entry (internal soundness fix, no behavior change).
3. Move this plan to `tasks/completed/` when done.

## Files to Modify
- `rust/tracing/src/logs/log_events.rs`
- `CHANGELOG.md`

## Decisions
- The `as usize` / `as u64` / `int as *const T` sites (`LogStringEvent`, `PropertySetId`) are
  already exposed-provenance and are left alone; the lint fires only on `transmute`.

## Trade-offs
- `with_exposed_provenance` vs strict `ptr::with_addr`: strict provenance can't work here
  because the pointer is rebuilt from an address stored in a byte buffer with no pointer to
  derive from, so the exposed-provenance model is the right fit.

## Testing Strategy
- `rust/tracing/tests` (`property_macro_tests.rs`, `dyn_deps_tests.rs`) exercise
  `read_value`; `rust/analytics/tests/log_tests.rs` only covers `write_value` and the wire
  format (it parses via the transit parser and never calls `read_value`). All must pass
  unchanged.
- Add a no-DB unit test in a new `#[cfg(test)]` module in `log_events.rs`: build a `static`
  `LogMetadata` (see `rust/analytics/tests/log_tests.rs` ~l.251), write a `TaggedLogString`,
  call `read_value`, and assert `std::ptr::eq` on `desc` and equality of the `properties`
  id, `time`, and `msg`.
- `cargo clippy --workspace -- -D warnings` must pass with the `allow` removed.

## Manual Verification
Run `cargo +nightly miri test -p micromegas-tracing` on the new unit test if Miri is
installed. Not automated in CI because Miri needs a nightly toolchain and is slow; expected
result: no provenance errors.
