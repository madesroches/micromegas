# Fix opt_uuid_from_string Empty-String Backward Compatibility

GitHub issue: #850

## Overview

The inline `uuid_serde::opt_uuid_from_string` in `rust/tracing/src/process_info.rs` was added for WASM support (transit is native-only) but dropped the empty-string → `None` handling that the original `transit::uuid_utils::opt_uuid_from_string` had. This causes 0.20.0 clients (which serialize `parent_process_id: None` as `""`) to get 400 errors from servers built at HEAD.

## Current State

**New inline version** (`rust/tracing/src/process_info.rs:34-45`):
- Serializes `None` as `serialize_none()` (CBOR null)
- Deserializes: `Some("")` → `try_parse("")` → **fails**

**Old transit version** (`rust/transit/src/uuid_utils.rs:15-29`):
- Serializes `None` as `serialize_str("")` (empty string)
- Deserializes: `Some("")` → `Ok(None)` (empty-string guard)

The ingestion server deserializes ProcessInfo from CBOR at `rust/ingestion/src/web_ingestion_service.rs:158`. When a 0.20.0 client sends `parent_process_id: ""`, the new code fails with `"failed to parse a UUID"`.

## Design

Add the empty-string guard back to the inline `opt_uuid_from_string`:

```rust
pub fn opt_uuid_from_string<'de, D>(deserializer: D) -> Result<Option<uuid::Uuid>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => uuid::Uuid::try_parse(&s)
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}
```

The serialization side (`opt_uuid_to_string`) does not need to change. The new format (`serialize_none`) is valid and the transit deserializer handles `None` correctly. We only need the deserialization side to accept both formats.

## Implementation Steps

1. Edit `rust/tracing/src/process_info.rs` line 39-44: add `Some(s) if s.is_empty() => Ok(None)` guard
2. Add a unit test for ProcessInfo round-trip with empty-string `parent_process_id` in `rust/tracing/tests/`
3. Run `cargo test` and `cargo fmt` from `rust/`

## Files to Modify

- `rust/tracing/src/process_info.rs` — add empty-string guard to `opt_uuid_from_string`
- `rust/tracing/tests/` — new test file for ProcessInfo serde round-trip

## Testing Strategy

- Unit test: serialize a ProcessInfo with `parent_process_id: None` using the old empty-string format (manually construct CBOR with `""` for the field), deserialize with the new code, assert `parent_process_id` is `None`
- Unit test: round-trip with the new format (`serialize_none`) still works
- Unit test: round-trip with a valid UUID in `parent_process_id` still works
- `cargo test` passes
- `cargo clippy --workspace -- -D warnings` passes
