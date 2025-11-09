# Pointer Provenance Fix for TaggedLogString Serialization

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/575

## Problem
In `rust/tracing/src/logs/events.rs`, `TaggedLogString` serializes pointer references as raw `u64` values and deserializes them back to references using `transmute`. This violates Rust's pointer provenance rules.

### Current Implementation
```rust
// Write side (line 276-280)
fn write_value(&self, buffer: &mut Vec<u8>) {
    write_any(buffer, &self.desc);        // Writes pointer as raw bytes
    write_any(buffer, &self.properties);  // Writes pointer as raw bytes
    write_any(buffer, &self.time);
    self.msg.write_value(buffer);
}

// Read side (line 284-295)
unsafe fn read_value(mut window: &[u8]) -> Self {
    let desc_id: u64 = read_consume_pod(&mut window);
    let properties_id: u64 = read_consume_pod(&mut window);
    // ...
    Self {
        desc: unsafe { std::mem::transmute::<u64, &LogMetadata<'static>>(desc_id) },
        properties: unsafe { std::mem::transmute::<u64, &PropertySet>(properties_id) },
        // ...
    }
}
```

### Why It's Wrong
- `write_any` writes the raw pointer bytes without calling `expose_provenance()`
- `read_value` reconstructs pointers using `transmute` instead of `with_exposed_provenance()`
- This is technically undefined behavior according to Rust's memory model
- Currently silenced with `#[allow(integer_to_ptr_transmutes)]` on line 283

## Solution
Fix both the write and read sides to properly handle provenance:

1. **Write side**: Use `expose_provenance()` when converting pointer to integer:
```rust
fn write_value(&self, buffer: &mut Vec<u8>) {
    let desc_addr = (self.desc as *const LogMetadata).expose_provenance() as u64;
    let props_addr = (self.properties as *const PropertySet).expose_provenance() as u64;
    write_any(buffer, &desc_addr);
    write_any(buffer, &props_addr);
    write_any(buffer, &self.time);
    self.msg.write_value(buffer);
}
```

2. **Read side**: Use `with_exposed_provenance()` when converting integer to pointer:
```rust
unsafe fn read_value(mut window: &[u8]) -> Self {
    let desc_id: u64 = read_consume_pod(&mut window);
    let properties_id: u64 = read_consume_pod(&mut window);
    let time: i64 = read_consume_pod(&mut window);
    let msg = DynString(read_advance_string(&mut window).unwrap());
    Self {
        desc: unsafe { &*std::ptr::with_exposed_provenance::<LogMetadata<'static>>(desc_id as usize) },
        properties: unsafe { &*std::ptr::with_exposed_provenance::<PropertySet>(properties_id as usize) },
        time,
        msg,
    }
}
```

3. **Remove the allow attribute** on line 283

## Testing
- Ensure `cargo clippy --workspace -- -D warnings` passes
- Run `cargo test` to verify behavior unchanged
- Consider running under Miri if available: `cargo +nightly miri test`

## Files to Change
- `rust/tracing/src/logs/events.rs`
  - Modify `write_value()` method around line 276
  - Modify `read_value()` method around line 284
  - Remove `#[allow(integer_to_ptr_transmutes)]` attribute on line 283

## Note
The static assertion about pointer size (line 13) should remain:
```rust
const _: () = assert!(std::mem::size_of::<usize>() == 8);
```
This ensures the code only compiles on 64-bit platforms where `u64 ↔ usize` conversions are safe.
