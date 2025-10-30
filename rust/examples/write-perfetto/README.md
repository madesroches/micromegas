# Trace Generation Utility

A command-line utility to generate Perfetto trace files from the Micromegas analytics service. This utility connects to the FlightSQL service, queries thread spans and async events, and produces a Perfetto trace file that can be viewed in the Perfetto UI.

## Features

- **Thread spans**: Queries and includes thread-based spans from the `thread_spans` view
- **Async spans**: Queries and includes async span events from the `async_events` view  
- **Single async track**: All async spans are placed on a unified "Async Operations" track
- **Flexible time range**: Supports custom time ranges or automatic process lifetime detection
- **Streaming generation**: Uses the streaming Perfetto writer for memory efficiency

## Usage

```bash
cargo run --package trace-gen-util --bin trace-gen -- [OPTIONS] --process-id <PROCESS_ID>
```

### Options

- `-p, --process-id <PROCESS_ID>`: Process ID to generate trace for (required)
- `-o, --output <OUTPUT>`: Output Perfetto trace file path (default: "trace.perfetto")
- `--flightsql-url <FLIGHTSQL_URL>`: FlightSQL server URL (default: "http://127.0.0.1:50051")
- `--start-time <START_TIME>`: Start time for trace (RFC 3339 format, optional)
- `--end-time <END_TIME>`: End time for trace (RFC 3339 format, optional)

### Examples

Generate trace for a specific process:
```bash
cargo run --bin trace-gen -- --process-id "my-process-123"
```

Generate trace with custom output file:
```bash
cargo run --bin trace-gen -- --process-id "my-process-123" --output "my-trace.perfetto"
```

Generate trace for specific time range:
```bash
cargo run --bin trace-gen -- --process-id "my-process-123" \
  --start-time "2024-01-01T00:00:00Z" \
  --end-time "2024-01-01T01:00:00Z"
```

Connect to different FlightSQL server:
```bash
cargo run --bin trace-gen -- --process-id "my-process-123" \
  --flightsql-url "http://localhost:9090"
```

## Viewing Traces

After generating a trace file, you can view it in the Perfetto UI:

1. Open https://ui.perfetto.dev in your browser
2. Click "Open trace file" 
3. Select the generated `.perfetto` file
4. Explore thread spans on individual thread tracks
5. View async operations on the unified "Async Operations" track

## Prerequisites

- Running Micromegas analytics services:
  - FlightSQL service (default port 50051)
  - Analytics data with process and span information
- Process must have telemetry data in the analytics system

## Implementation Notes

- Uses the Phase 3 async span support in the Perfetto writer library
- Queries `view_instance('thread_spans', stream_id)` for thread spans
- Queries `view_instance('async_events', process_id)` for async span events
- Creates proper track hierarchy: Process → Thread tracks + Async track
- All async spans appear on a single track for better visualization
- Handles both "begin" and "end" async events properly