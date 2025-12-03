# Unreal Engine Integration

Micromegas provides high-performance observability for Unreal Engine applications through a native integration that captures logs, metrics, and traces with minimal overhead.

## Overview

The Unreal Engine integration consists of:

- **MicromegasTracing**: Extension to Unreal's Core module providing logging, metrics, and span tracking
- **MicromegasTelemetrySink**: Plugin adding HTTP transport for sending telemetry to the ingestion service

## Key Features

- **Low Overhead**: ~20ns per event, matching the Rust implementation's performance
- **Seamless Integration**: Automatically captures existing UE_LOG statements
- **Simple Setup**: One header file to include: `#include "MicromegasTracing/Macros.h"`
- **Comprehensive Telemetry**: Logs, metrics, spans, and crash reporting in a unified system
- **Thread-Safe**: Asynchronous delivery without blocking the game thread
- **Context Propagation**: Global properties automatically attached to all telemetry

## Quick Start

### 1. Install the Plugin

Copy the Unreal modules to your project:
- `unreal/MicromegasTracing` → Your project's Source folder
- `unreal/MicromegasTelemetrySink` → Your project's Plugins folder

### 2. Initialize Telemetry

In your GameInstance or GameMode:

```cpp
#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "MicromegasTelemetrySink/ApiKeyAuthenticator.h"

void AMyGameMode::BeginPlay()
{
    Super::BeginPlay();

    // Initialize telemetry with API key authentication
    FString ApiKey = TEXT("your-api-key");  // Store securely!
    auto AuthProvider = MakeShared<FApiKeyAuthenticator>(ApiKey);

    IMicromegasTelemetrySinkModule::LoadModuleChecked().InitTelemetry(
        "https://your-telemetry-server:9000",  // Your ingestion server
        AuthProvider
    );
}
```

### 3. Add Instrumentation

```cpp
#include "MicromegasTracing/Macros.h"

void AMyActor::Tick(float DeltaTime)
{
    // Trace function execution
    MICROMEGAS_SPAN_FUNCTION("Game");
    
    // Log an event
    MICROMEGAS_LOG("Game", MicromegasTracing::LogLevel::Info, 
                   TEXT("Actor ticking"));
    
    // Record a metric
    MICROMEGAS_FMETRIC("Game", MicromegasTracing::Verbosity::Med, 
                       TEXT("TickTime"), TEXT("ms"), DeltaTime * 1000);
}
```

### 4. View Your Data

Once your game is running and generating telemetry:

- **Query your data**: Follow the [Query Guide](../query-guide/index.md) to learn SQL querying and Python API usage
- **Visualize traces**: Generate Perfetto traces for detailed performance analysis
- **Build dashboards**: Create custom analytics and monitoring dashboards

## What Gets Captured

### Automatic Telemetry

- **UE_LOG statements**: All existing Unreal logs are automatically captured
- **Frame metrics**: Delta time, frame rate (when MetricPublisher is active)
- **Memory metrics**: Physical and virtual memory usage
- **Map changes**: Current level/world tracked in context
- **Crashes**: Stack traces and context on Windows (requires debug symbols)

### Manual Instrumentation

- **Custom spans**: Track specific operations and their duration
- **Business metrics**: Player counts, game state, performance indicators
- **Custom logs**: Direct telemetry logging with structured properties
- **Context properties**: Session IDs, user IDs, build versions

## Architecture

```
Game Code
    ↓
[MicromegasTracing Module]
    ├─ Logging API
    ├─ Metrics API
    ├─ Spans API
    └─ Default Context
         ↓
[MicromegasTelemetrySink Plugin]
    ├─ HTTP Event Sink
    ├─ Flush Monitor
    ├─ Sampling Controller
    └─ Crash Reporter
         ↓
[Telemetry Ingestion Server]
    ├─ PostgreSQL (metadata)
    └─ Object Storage (payloads)
```

## Next Steps

- [Installation Guide](installation.md) - Detailed setup instructions
- [Instrumentation API](instrumentation-api.md) - Complete API reference
- [Examples](examples.md) - Common instrumentation patterns

## Performance Considerations

- Spans are disabled by default (enable with `telemetry.spans.enable 1`)
- Events are buffered in thread-local storage before async delivery
- The HTTP sink runs on a dedicated thread
- Use sampling for high-frequency spans to manage data volume
- Default context operations are expensive - use for infrequent changes

## Platform Support

- **Windows**: Full support including crash reporting
- **Linux**: Full support
- **Mac**: Full support
- **Consoles**: Requires network configuration
- **Mobile**: Consider battery and bandwidth optimization