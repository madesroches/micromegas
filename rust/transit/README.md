# micromegas-transit

Fast binary serialization for Plain Old Data structures. Values are copied memcpy-style from `#[repr(C)]` structs, and `Reflect` / `UserDefinedType` metadata describes their layout, so a reader can parse a heterogeneous event queue without having the writer's types.

Use it for high-throughput, in-process event buffers. It is not a general schema-evolution format.

## Example

```rust
use micromegas_transit::{read_consume_pod, write_any};

let mut buffer = Vec::new();
write_any(&mut buffer, &42u32);
write_any(&mut buffer, &7u64);

let mut window = &buffer[..];
let a: u32 = read_consume_pod(&mut window);
let b: u64 = read_consume_pod(&mut window);
assert_eq!((a, b), (42, 7));
assert!(window.is_empty());
```

## Documentation

- [API documentation on docs.rs](https://docs.rs/micromegas-transit)
- [GitHub Repository](https://github.com/madesroches/micromegas)
- [Micromegas Home Page](https://micromegas.info/)
