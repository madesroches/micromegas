# Log Forwarder Console Application Plan

## Overview
Create a console application that executes given command-line arguments, captures their output (stdout/stderr), and forwards the output as structured log entries to the Micromegas ingestion service.

## Application Name
`fax`

## Core Requirements

### Command Execution
- Accept command and arguments via CLI arguments
- Execute the command using `std::process::Command`
- Capture both stdout and stderr streams
- Preserve exit codes and relay them appropriately
- Handle real-time output streaming (don't buffer entire output)

### Log Processing
- Parse output line-by-line
- Create structured log entries with metadata:
  - Timestamp (when line was captured)
  - Source command and arguments
  - Stream type (stdout/stderr)
  - Process PID
  - Exit code (when available)
  - Line number/sequence
- Support different log levels based on stream (stdout=info, stderr=warn/error)

### Ingestion Integration
- Use existing `micromegas-tracing` or `telemetry-sink` for sending logs
- Configure ingestion service endpoint via environment variables
- Handle connection failures gracefully (buffer/retry or fail fast)
- Support batch sending for performance

### Configuration
- Environment variables:
  - `MICROMEGAS_INGESTION_URL`: Ingestion service endpoint
- CLI options:
  - `--service-name`: Override service name in logs
  - `--no-forward`: Execute command without forwarding (dry-run mode)

## Technical Design

### Project Structure
```
rust/
├── Cargo.toml (add new binary)
└── fax/
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs
    │   ├── command.rs      # Command execution logic
    │   ├── log_processor.rs # Log parsing and structuring
    │   └── forwarder.rs    # Ingestion service integration
    └── README.md
```

### Key Dependencies
- `tokio` for async runtime and process handling
- `micromegas-tracing` for log emission
- `clap` for CLI argument parsing
- `serde_json` for structured log serialization
- `tracing` for internal application logging

### Implementation Phases

#### Phase 1: Basic Command Execution
- CLI parsing with clap
- Execute command and capture output
- Basic stdout/stderr handling
- Exit code preservation

#### Phase 2: Log Processing
- Line-by-line output processing
- Structured log entry creation
- Timestamp and metadata addition
- Stream differentiation

#### Phase 3: Ingestion Integration
- Integrate with micromegas-tracing
- Configure ingestion endpoint
- Basic error handling
- Real-time log forwarding

#### Phase 4: Advanced Features
- Buffering and batching
- Retry logic for failed sends
- Configuration file support
- Enhanced metadata (process info, environment)

## Usage Examples

```bash
# Basic usage
fax ls -la /home

# With service name override
fax --service-name my-backup-service rsync -av /src/ /dest/

# Dry run mode
fax --no-forward docker build .

# Long-running process
fax tail -f /var/log/application.log
```

## Integration Points
- Uses existing ingestion service HTTP endpoint
- Leverages micromegas-tracing for consistent log format
- Follows existing Rust project conventions
- Integrates with current service management scripts

## Testing Strategy
- Unit tests for log processing logic
- Integration tests with mock ingestion service
- End-to-end tests with actual commands
- Performance tests with high-volume output

## Encoding and Character Set Handling

### Research Summary
The application needs to handle different character encodings from shell commands, as output encoding depends on:

1. **Environment Variables**: `LC_ALL`, `LC_CTYPE`, `LANG` determine the locale and encoding
2. **System Default**: Most modern Linux systems default to UTF-8
3. **Legacy Systems**: May use ISO-8859-1, KOI8-R, or other regional encodings
4. **Command-Specific**: Some commands may output in specific encodings regardless of locale

### Detection Strategy
```rust
fn detect_encoding() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "C.UTF-8".to_string())
}
```

### Implementation Requirements
- **Input Handling**: Capture raw bytes from command output (don't assume UTF-8)
- **Encoding Detection**: Use locale environment variables to determine expected encoding
- **Transcoding**: Convert non-UTF-8 output to UTF-8 for consistent log storage
- **Error Handling**: Handle invalid/mixed encodings gracefully (replacement characters)
- **Metadata**: Include detected/assumed encoding in log metadata

### Rust Dependencies
- `encoding_rs` or `iconv` for character set conversion
- Handle both valid UTF-8 and legacy encoding scenarios
- Preserve original bytes for debugging if transcoding fails

## Security Considerations
- Sanitize command arguments in logs
- Avoid logging sensitive environment variables
- Secure ingestion service authentication
- Rate limiting to prevent ingestion service overload