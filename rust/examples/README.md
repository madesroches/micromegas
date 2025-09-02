# Examples

This directory contains example utilities and tools for development and testing purposes.

## write-perfetto

**For testing and development only**

The `write-perfetto` contains utilities for writing and validating Perfetto traces from the analytics service. This is intended for internal testing and development workflows.

**Users should use the Python CLI scripts** in `python/micromegas/` to generate Perfetto traces for production use.

### Binaries

- `write-perfetto`: Writes Perfetto traces from analytics data
- `validate-perfetto`: Validates generated trace files

### Usage

```bash
# From the rust/ directory, you can build and run:
cargo run --bin write-perfetto -- [options]
cargo run --bin validate-perfetto -- [options]

# Or from the specific example directory:
cd rust/examples/write-perfetto
cargo run --bin write-perfetto -- [options]
cargo run --bin validate-perfetto -- [options]
```