# Task: Document Unreal Engine Instrumentation

## Objective
Create comprehensive documentation for instrumenting Unreal Engine applications with Micromegas telemetry, enabling developers to add observability to their UE projects.

## Background
The Micromegas Unreal integration consists of:
- **MicromegasTracing**: Extension to Unreal's Core module (logs, metrics, spans)
- **MicromegasTelemetrySink**: Plugin that adds HTTP transport for sending telemetry to the ingestion service

## Documentation Structure

### 1. Introduction & Overview
- **Purpose**: Explain what Micromegas provides for Unreal Engine developers
- **Architecture**: Extension to Core module with plugin for HTTP transport
- **Key Benefits**: 
  - Low overhead (~20ns per event, similar to Rust implementation)
  - Seamless UE logging system integration (UE_LOG automatically captured)
  - Built-in crash reporting (Windows)
  - Thread-safe async delivery
  - Works with existing UE_LOG statements without modification

### 2. Plugin Installation

#### Standard Installation
- **Steps**:
  - Copy `unreal/MicromegasTracing` (core module) to project's Source folder or as a module
  - Copy `unreal/MicromegasTelemetrySink` (plugin) to project's Plugins folder
  - Add plugin to .uproject file or enable via Unreal Editor
  - Configure Build.cs module dependencies

#### Development Setup (Windows)
- **For active development on Micromegas while testing in Unreal**:
  - Use `build/unreal_hard_link_windows.py` script to create hard links
  - Set environment variables:
    - `MICROMEGAS_UNREAL_ROOT_DIR`: Path to Unreal Engine root
    - `MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR`: Path to your project's plugin directory
  - Run the script to create symlinks that allow live development without copying files
  - This enables testing changes immediately without manual file copying

### 3. Initial Setup & Configuration
- **Initialization Code**:
  ```cpp
  // In GameInstance or GameMode
  IMicromegasTelemetrySinkModule::LoadModuleChecked().InitTelemetry(
      "https://telemetry.example.com:9000",  // Ingestion server URL
      AuthenticationProvider                  // Authentication handler
  );
  ```
- **Console Commands**:
  - `telemetry.enable` - Initialize telemetry system
  - `telemetry.flush` - Force flush pending events
- **Environment Variables**: Configuration for dev/staging/production

### 4. Core Instrumentation APIs

#### 4.1 Logging
- **UE_LOG Integration**: Existing UE_LOG statements are automatically captured (convenient but less efficient)
- **Direct Logging**: `MICROMEGAS_LOG(target, level, message)` - For direct telemetry logging
- **Structured Logging**: `MICROMEGAS_LOG_PROPERTIES(target, level, properties, message)` - Include additional properties
- **Log Levels**: Fatal, Error, Warn, Info, Debug, Trace (from `MicromegasTracing::LogLevel`)
- **Examples**:
  ```cpp
  // Basic logging
  MICROMEGAS_LOG("MicromegasTelemetrySink", MicromegasTracing::LogLevel::Info, TEXT("Shutting down"));
  
  // Dynamic string with formatting
  MICROMEGAS_LOG("LogMicromegasTelemetrySink", MicromegasTracing::LogLevel::Debug, 
                 FString::Printf(TEXT("Sending block %s"), *blockId));
  
  // With properties
  MICROMEGAS_LOG_PROPERTIES("Game", MicromegasTracing::LogLevel::Info, properties, TEXT("Player joined"));
  ```

#### 4.2 Spans/Tracing
- **Important**: Spans are **NOT enabled by default** - must be explicitly enabled
- **Function Tracing**: `MICROMEGAS_SPAN_FUNCTION(target)` - Uses function name as span name
- **Named Scopes**: `MICROMEGAS_SPAN_SCOPE(target, name)` - Static name for the span
- **Dynamic Names**: `MICROMEGAS_SPAN_NAME(target, variable_name)` - Expression returning **statically allocated** string
  - Works with FNames
  - Example: `MICROMEGAS_SPAN_NAME("Engine::FActorTickFunction::ExecuteTick", Target->GetFName())`
- **UObject Spans**: `MICROMEGAS_SPAN_UOBJECT(target, actor/component)`
- **Conditional Spans**: `MICROMEGAS_SPAN_NAME_CONDITIONAL(target, condition, name)`
- **Console Commands for Spans**:
  ```
  telemetry.spans.enable 1  // Enable span recording
  telemetry.flush           // Flush pending spans
  telemetry.spans.enable 0  // Disable span recording
  ```

#### 4.3 Metrics
- **Integer Metrics**: `MICROMEGAS_IMETRIC(target, level, name, unit, expression)`
- **Float Metrics**: `MICROMEGAS_FMETRIC(target, level, name, unit, expression)`
- **Common Units**: "ms", "bytes", "count", "fps", "percent", "seconds"
- **Example**:
  ```cpp
  MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("DeltaTime"), TEXT("seconds"), FApp::GetDeltaTime());
  ```

### 5. Common Instrumentation Patterns

#### 5.1 Game Loop Instrumentation
```cpp
// Frame timing
MICROMEGAS_SPAN_FUNCTION("Game.Frame");
MICROMEGAS_FMETRIC("Game.Performance", MicromegasTracing::Verbosity::Med, TEXT("FrameTime"), TEXT("ms"), DeltaTime * 1000);
```

#### 5.2 Actor/Component Lifecycle
```cpp
void AMyActor::BeginPlay() {
    MICROMEGAS_SPAN_UOBJECT("Game.Actor", this);
    MICROMEGAS_LOG("Game.Actor", MicromegasTracing::LogLevel::Info, FString::Printf(TEXT("Actor %s spawned"), *GetName()));
}
```

#### 5.3 Network Replication
```cpp
MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med, TEXT("ReplicatedActors"), TEXT("count"), GetNetDriver()->ClientConnections.Num());
```

#### 5.4 Asset Loading
```cpp
MICROMEGAS_SPAN_NAME("Content.Loading", AssetPath);
MICROMEGAS_IMETRIC("Content", MicromegasTracing::Verbosity::Med, TEXT("AssetSize"), TEXT("bytes"), Asset->GetResourceSizeBytes());
```

### 6. Performance Profiling Setup
- **CPU Profiling**:
  - Game thread vs Render thread separation
  - Task graph instrumentation
  - Blueprint VM overhead tracking
- **GPU Profiling**:
  - Draw call metrics
  - Shader compilation tracking
  - Texture memory usage
- **Memory Profiling**:
  - Allocation tracking
  - GC metrics
  - Asset memory footprint

### 7. Advanced Features

#### Default Context
- **Purpose**: Set infrequently changing properties that are automatically attached to ALL metrics and log entries
- **Best For**: Session IDs, build versions, current map/level, user IDs - properties that change rarely
- **Usage**: Access via `MicromegasTracing::Dispatch::GetDefaultContext()`

##### Common Usage Pattern
```cpp
// In GameInstance or GameMode initialization
MicromegasTracing::DefaultContext* Ctx = MicromegasTracing::Dispatch::GetDefaultContext();
if (Ctx) {
    // Set persistent properties for the session
    Ctx->Set(FName("session_id"), FName(*FGuid::NewGuid().ToString()));
    Ctx->Set(FName("build_version"), FName(TEXT("1.2.3")));
    Ctx->Set(FName("platform"), FName(UGameplayStatics::GetPlatformName()));
    Ctx->Set(FName("user_id"), FName(*GetPlayerID()));
    
    // Map is automatically updated by MetricPublisher, but you can set it manually
    Ctx->Set(FName("map"), FName(TEXT("MainMenu")));
}

// Later, when changing levels
if (Ctx) {
    Ctx->Set(FName("map"), FName(*NewWorldName));
}

// To remove a property
Ctx->Unset(FName("session_id"));

// To clear all properties (rarely needed)
Ctx->Clear();
```

- **Automatic Map Tracking**: The MetricPublisher automatically updates the "map" property when the world changes
- **Thread-Safe**: Can be called from any thread, but Set/Unset/Clear are expensive operations
- **Important**: Keys and values are never freed, so limit cardinality to avoid memory issues

#### Other Features
- **Sampling Controller**: Reduce overhead for high-frequency events
- **Property Sets**: Additional contextual data for specific scopes beyond the default context
- **Flush Monitor**: Automatically flushes queued events at regular intervals
- **Thread Streams**: Per-thread event buffering for minimal contention
- **Crash Reporting**: Automatic context capture on crashes (Windows) - requires access to debug symbols

### 8. Build & Packaging Considerations
- **Conditional Compilation**:
  - Control telemetry initialization: `#define MICROMEGAS_ENABLE_TELEMETRY_ON_START 1` (or 0 to disable)
  - For shipping builds, consider wrapping instrumentation in custom macros that can be compiled out
- **Shipping Builds**: Strip telemetry or reduce verbosity
- **Platform Specific**:
  - Windows: Crash reporting enabled
  - Console platforms: Special network considerations
  - Mobile: Battery and bandwidth optimization

## MkDocs Implementation Status

### Files Created ✅
1. **`mkdocs/docs/unreal/index.md`** - Main Unreal Engine integration overview
   - ✅ Introduction to Micromegas for Unreal
   - ✅ Architecture overview (core module + plugin)
   - ✅ Quick start guide with complete setup example
   - ✅ Key features and platform support
   
2. **`mkdocs/docs/unreal/installation.md`** - Detailed installation guide
   - ✅ Standard installation steps with Build.cs configuration
   - ✅ Development setup with hard links script (Windows)
   - ✅ Authentication provider implementation
   - ✅ Console commands and verification steps
   - ✅ Platform-specific notes and troubleshooting
   
3. **`mkdocs/docs/unreal/instrumentation-api.md`** - Complete API reference
   - ✅ Logging macros (MICROMEGAS_LOG, MICROMEGAS_LOG_PROPERTIES)
   - ✅ Span/tracing macros (MICROMEGAS_SPAN_FUNCTION, MICROMEGAS_SPAN_NAME, etc.)
   - ✅ Metric macros (MICROMEGAS_IMETRIC, MICROMEGAS_FMETRIC) with correct verbosity levels
   - ✅ Default Context API with full examples
   - ✅ Console commands reference
   - ✅ Best practices and thread safety notes
   
4. **`mkdocs/docs/unreal/examples.md`** - Comprehensive practical examples
   - ✅ Complete GameInstance initialization with context setup
   - ✅ Game loop instrumentation with performance metrics
   - ✅ Actor lifecycle tracking and component instrumentation
   - ✅ Network replication metrics and RPC tracking
   - ✅ AI and Behavior Tree instrumentation
   - ✅ Asset loading and streaming telemetry
   - ✅ Error handling and debugging patterns
   - ✅ Development vs production configuration

### Files Modified ✅
1. **`mkdocs/mkdocs.yml`** - Added Unreal section to navigation
   ```yaml
   nav:
     - Unreal Engine:
       - Overview: unreal/index.md
       - Installation: unreal/installation.md
       - Instrumentation API: unreal/instrumentation-api.md
       - Examples: unreal/examples.md
   ```

2. **`mkdocs/docs/index.md`** - Updated main documentation index
   - ✅ Added Unreal Engine to quick start section
   - ✅ Added Game Development use case

3. **`mkdocs/docs/getting-started.md`** - Updated next steps
   - ✅ Added Unreal Engine integration as primary next step

### Final Documentation Structure
```
mkdocs/docs/
├── index.md (modified ✅)
├── getting-started.md (modified ✅)
└── unreal/ (new ✅)
    ├── index.md ✅
    ├── installation.md ✅
    ├── instrumentation-api.md ✅
    └── examples.md ✅
```

### Implementation Notes

- **API Accuracy**: All code samples verified against actual codebase implementation
- **Macro Corrections**: Fixed `MICROMEGAS_LOG_STATIC/DYNAMIC` to actual `MICROMEGAS_LOG` API
- **Verbosity Levels**: Corrected metrics to use `MicromegasTracing::Verbosity` instead of `LogLevel`
- **Default Context**: Comprehensive coverage of global context features and usage patterns
- **Console Commands**: Verified all commands exist in the codebase
- **Development Workflow**: Included Windows hard link script for active development
- **Platform Coverage**: Added specific notes for Windows, Linux, Mac, console, and mobile platforms
- **Performance Guidance**: Emphasized span enablement requirements and overhead considerations

### Ready for Deployment

The documentation is complete and ready for use. Users can now:
1. Navigate to the Unreal Engine section from the main documentation
2. Follow step-by-step installation instructions
3. Reference the complete API with verified code samples  
4. Use comprehensive examples for common game development scenarios
