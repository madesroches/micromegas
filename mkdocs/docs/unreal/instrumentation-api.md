# Instrumentation API Reference

Complete reference for the Micromegas Unreal Engine instrumentation API.

## Header File

All instrumentation macros are available by including:

```cpp
#include "MicromegasTracing/Macros.h"
```

## Logging API

### MICROMEGAS_LOG

Records a log entry with dynamic string content.

```cpp
MICROMEGAS_LOG(target, level, message)
```

**Parameters:**

- `target` (const char*): Log target/category (e.g., "Game", "Network", "AI")
- `level` (MicromegasTracing::LogLevel): Severity level
- `message` (FString): The log message

**Log Levels:**

- `MicromegasTracing::LogLevel::Fatal` - Critical errors causing shutdown
- `MicromegasTracing::LogLevel::Error` - Errors requiring attention
- `MicromegasTracing::LogLevel::Warn` - Warning conditions
- `MicromegasTracing::LogLevel::Info` - Informational messages
- `MicromegasTracing::LogLevel::Debug` - Debug information
- `MicromegasTracing::LogLevel::Trace` - Detailed trace information

**Example:**
```cpp
MICROMEGAS_LOG("Game", MicromegasTracing::LogLevel::Info, 
               TEXT("Player connected"));

MICROMEGAS_LOG("Network", MicromegasTracing::LogLevel::Error,
               FString::Printf(TEXT("Connection failed: %s"), *ErrorMessage));
```

### MICROMEGAS_LOG_PROPERTIES

Records a log entry with additional structured properties.

```cpp
MICROMEGAS_LOG_PROPERTIES(target, level, properties, message)
```

**Parameters:**

- `target` (const char*): Log target/category
- `level` (MicromegasTracing::LogLevel): Severity level
- `properties` (PropertySet*): Additional key-value properties
- `message` (FString): The log message

**Example:**
```cpp
PropertySet* Props = CreatePropertySet();
Props->Add("player_id", "12345");
Props->Add("action", "login");

MICROMEGAS_LOG_PROPERTIES("Game", MicromegasTracing::LogLevel::Info, 
                         Props, TEXT("Player action recorded"));
```

### UE_LOG Integration

All existing `UE_LOG` statements are automatically captured by Micromegas when the log interop is initialized. No code changes required.

```cpp
// These are automatically sent to telemetry
UE_LOG(LogTemp, Warning, TEXT("This is captured by Micromegas"));
UE_LOG(LogGameMode, Error, TEXT("So is this"));
```

## Metrics API

### MICROMEGAS_IMETRIC

Records an integer metric value.

```cpp
MICROMEGAS_IMETRIC(target, level, name, unit, expression)
```

**Parameters:**

- `target` (const char*): Metric target/category
- `level` (MicromegasTracing::Verbosity): Verbosity level
- `name` (const TCHAR*): Metric name
- `unit` (const TCHAR*): Unit of measurement
- `expression` (int64): Value or expression to record

**Verbosity Levels:**

- `MicromegasTracing::Verbosity::Low` - Critical metrics only
- `MicromegasTracing::Verbosity::Med` - Standard metrics
- `MicromegasTracing::Verbosity::High` - Detailed metrics

**Common Units:**

- `TEXT("count")` - Simple counter
- `TEXT("bytes")` - Memory/data size
- `TEXT("ms")` - Milliseconds
- `TEXT("percent")` - Percentage (0-100)
- `TEXT("ticks")` - Will be automatically converted into nanoseconds

**Example:**
```cpp
MICROMEGAS_IMETRIC("Game", MicromegasTracing::Verbosity::Med,
                   TEXT("PlayerCount"), TEXT("count"), 
                   GetWorld()->GetNumPlayerControllers());

MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Low,
                   TEXT("TextureMemory"), TEXT("bytes"),
                   GetTextureMemoryUsage());
```

### MICROMEGAS_FMETRIC

Records a floating-point metric value.

```cpp
MICROMEGAS_FMETRIC(target, level, name, unit, expression)
```

**Parameters:**

- `target` (const char*): Metric target/category
- `level` (MicromegasTracing::Verbosity): Verbosity level
- `name` (const TCHAR*): Metric name
- `unit` (const TCHAR*): Unit of measurement
- `expression` (double): Value or expression to record

**Example:**
```cpp
MICROMEGAS_FMETRIC("Performance", MicromegasTracing::Verbosity::Med,
                   TEXT("FrameTime"), TEXT("ms"), 
                   DeltaTime * 1000.0);

MICROMEGAS_FMETRIC("Game", MicromegasTracing::Verbosity::High,
                   TEXT("HealthPercent"), TEXT("percent"),
                   (Health / MaxHealth) * 100.0);
```

## Spans/Tracing API

**Important:** Spans are **disabled by default**. Enable with console command: `telemetry.spans.enable 1`. Use reasonable sampling strategy for high-frequency spans.

### MICROMEGAS_SPAN_FUNCTION

Traces the current function's execution time using the function name as the span name.

```cpp
MICROMEGAS_SPAN_FUNCTION(target)
```

**Parameters:**

- `target` (const char*): Span target/category

**Example:**
```cpp
void AMyActor::ComplexCalculation()
{
    MICROMEGAS_SPAN_FUNCTION("Game.Physics");
    // Function is automatically traced
    // ... complex physics calculations ...
}
```

### MICROMEGAS_SPAN_SCOPE

Creates a named scope span with a static name.

```cpp
MICROMEGAS_SPAN_SCOPE(target, name)
```

**Parameters:**

- `target` (const char*): Span target/category
- `name` (const char*): Static span name

**Example:**
```cpp
void ProcessAI()
{
    {
        MICROMEGAS_SPAN_SCOPE("AI", "Pathfinding");
        // ... pathfinding code ...
    }
    
    {
        MICROMEGAS_SPAN_SCOPE("AI", "DecisionTree");
        // ... decision tree evaluation ...
    }
}
```

### MICROMEGAS_SPAN_NAME

Creates a span with a dynamic name (must be statically allocated).

```cpp
MICROMEGAS_SPAN_NAME(target, name_expression)
```

**Parameters:**

- `target` (const char*): Span target/category
- `name_expression`: Expression returning a statically allocated string (e.g., FName)

**Example:**
```cpp
void ProcessAsset(const FString& AssetPath)
{
    FName AssetName(*AssetPath);
    MICROMEGAS_SPAN_NAME("Content", AssetName);
    // ... process asset ...
}
```

### MICROMEGAS_SPAN_UOBJECT

Creates a span named after a UObject.

```cpp
MICROMEGAS_SPAN_UOBJECT(target, object)
```

**Parameters:**

- `target` (const char*): Span target/category
- `object` (UObject*): The UObject whose name to use

**Example:**
```cpp
void AMyActor::Tick(float DeltaTime)
{
    MICROMEGAS_SPAN_UOBJECT("Game.Actors", this);
    Super::Tick(DeltaTime);
    // ... tick logic ...
}
```

### MICROMEGAS_SPAN_NAME_CONDITIONAL

Creates a span conditionally.

```cpp
MICROMEGAS_SPAN_NAME_CONDITIONAL(target, condition, name)
```

**Parameters:**

- `target` (const char*): Span target/category
- `condition` (bool): Whether to create the span
- `name`: Span name if condition is true

**Example:**
```cpp
void RenderFrame(bool bDetailedProfiling)
{
    MICROMEGAS_SPAN_NAME_CONDITIONAL("Render", bDetailedProfiling, 
                                      TEXT("DetailedFrame"));
    // ... rendering code ...
}
```

## Network Tracing

!!! note "Audience"
    This section is written to be readable by both human integrators and coding-agent LLMs (e.g. Claude Code). Each macro entry follows a fixed shape — **signature**, **parameters**, **semantics**, **bit-source expression**, **example** — so an agent can apply each site without ambiguity.

For the engine-side recipe — which UE files to modify and where — see [Network Tracing](network-tracing.md).

Net-trace macros capture per-connection replication traffic with bit-size attribution. They live in the same header as logs/metrics/spans:

```cpp
#include "MicromegasTracing/Macros.h"
```

All macros are RAII where applicable; early returns close scopes automatically. When `MICROMEGAS_NET_TRACE_ENABLED` is `0`, every macro expands to nothing — **zero overhead**.

### Bit-source cheat sheet

Every scope macro that takes a `getBitsExpr` parameter captures the position at entry and measures the delta at exit. The expression must refer to a bit stream that's being written to (send) or read from (receive) inside the scope.

| Situation | Expression |
|-----------|-----------|
| Classic send (outgoing bunch) | `Bunch.GetNumBits()` |
| Classic receive (incoming reader) | `Reader.GetPosBits()` |
| Classic RPC send (`TempWriter`) | `TempWriter.GetNumBits()` |
| Classic fast-array property writer | `TempBitWriter.GetNumBits()` |
| Iris send | `Context.GetBitStreamWriter()->GetPosBits()` |
| Iris receive | `Context.GetBitStreamReader()->GetPosBits()` |

Flat property calls (`MICROMEGAS_NET_PROPERTY`) pass the pre-computed bit size directly — no bit stream expression needed.

### MICROMEGAS_NET_CONNECTION_SCOPE

Opens a per-connection scope. All object/property/RPC events emitted inside are attributed to this connection.

```cpp
MICROMEGAS_NET_CONNECTION_SCOPE(ConnectionName, bIsOutgoing)
```

**Parameters:**

- `ConnectionName` (FName): stable connection identifier (use `Connection->GetPlayerOnlinePlatformName()` with `GetFName()` fallback)
- `bIsOutgoing` (bool): `true` for send paths, `false` for receive paths

**Semantics:**

- RAII; closes on scope exit
- Connection scopes **do not nest** — only the outermost emits, inner ones are absorbed as no-ops and logged once via `LogMicromegasNet`
- Snapshots the current runtime verbosity at the outermost Begin; CVar changes take effect at the next outer scope

**Emits:** `NetConnectionBeginEvent` on entry, `NetConnectionEndEvent` (with `bit_size` = sum of root object/RPC bits) on exit.

**Example:**

```cpp
void UNetConnection::ReceivedPacket(FBitReader& Reader)
{
    FName MmConnectionName = GetPlayerOnlinePlatformName();
    if (MmConnectionName == NAME_None) { MmConnectionName = GetFName(); }
    MICROMEGAS_NET_CONNECTION_SCOPE(MmConnectionName, /*bIsOutgoing=*/ false);
    // ... packet processing ...
}
```

### MICROMEGAS_NET_OBJECT_SCOPE

Opens a per-object scope (root actor or subobject). Measures bit-stream delta from entry to exit.

```cpp
MICROMEGAS_NET_OBJECT_SCOPE(ObjectName, getBitsExpr)
```

**Parameters:**

- `ObjectName`: `FName` or `const TCHAR*` (anything acceptable to `StaticStringRef`)
- `getBitsExpr`: expression returning the current bit-stream position (see cheat sheet above)

**Semantics:**

- RAII; on destruction emits `NetObjectEndEvent` with `bit_size = GetBits() - StartBits`
- Depth 0 (root) requires verbosity ≥ `RootObjects`; depth 1+ requires ≥ `Objects`
- Classic emits subobjects as peers at depth 0; Iris emits them nested at depth 1+

**Emits:** `NetObjectBeginEvent` on entry, `NetObjectEndEvent` on exit.

**Example (classic send):**

```cpp
MICROMEGAS_NET_OBJECT_SCOPE(Actor->GetFName(), Bunch.GetNumBits());
```

**Example (Iris send, with null guard):**

```cpp
MICROMEGAS_NET_OBJECT_SCOPE(
    (ObjectData.Protocol && ObjectData.Protocol->DebugName) ? ObjectData.Protocol->DebugName->Name : TEXT("Unknown"),
    Context.GetBitStreamWriter()->GetPosBits());
```

### MICROMEGAS_NET_RPC_SCOPE

Opens a per-RPC scope. Same shape as object scope but emits `NetRPCBeginEvent` / `NetRPCEndEvent`.

```cpp
MICROMEGAS_NET_RPC_SCOPE(FunctionName, getBitsExpr)
```

**Parameters:**

- `FunctionName`: `FName` or `const TCHAR*`
- `getBitsExpr`: expression returning the current bit-stream position

**Semantics:**

- RAII; `EndRPC` applies an `ObjectDepth == 0` gate that prevents double-counting when an RPC fires inside an object scope (nested RPC bits roll into the outer object, not double-attributed to the connection)

**Example (classic send):**

```cpp
MICROMEGAS_NET_RPC_SCOPE(Function->GetFName(), TempWriter.GetNumBits());
```

**Example (Iris receive, post-resolve):**

```cpp
MICROMEGAS_NET_RPC_SCOPE(BlobDescriptor->DebugName->Name,
                        Context.GetBitStreamReader()->GetPosBits());
```

### MICROMEGAS_NET_PROPERTY

Flat property leaf — emits a single `NetPropertyEvent` with the pre-computed bit size. No Begin/End pair.

```cpp
MICROMEGAS_NET_PROPERTY(PropertyName, bitSize)
```

**Parameters:**

- `PropertyName`: `FName` or `const TCHAR*`
- `bitSize` (uint32): pre-computed bit length

**Semantics:**

- Gated at verbosity ≥ `Properties`
- Use when the bit size is already known (e.g. `SharedPropInfo->PropBitLength`, or a `NumEndBits - NumStartBits` delta captured for `NETWORK_PROFILER`)

**Example:**

```cpp
MICROMEGAS_NET_PROPERTY(Cmd.Property->GetFName(), NumEndBits - NumStartBits);
```

### MICROMEGAS_NET_PROPERTY_SCOPE

Scope-form property — measures the bit-stream delta across the wrapped serialize/deserialize call and emits a single `NetPropertyEvent` on destruction (still a leaf, no Begin/End pair).

```cpp
MICROMEGAS_NET_PROPERTY_SCOPE(PropertyName, getBitsExpr)
```

**Parameters:**

- `PropertyName`: `FName` or `const TCHAR*`
- `getBitsExpr`: bit-stream position expression (see cheat sheet)

**Semantics:**

- Use when no pre-computed bit size is available (Iris properties, classic receive paths)
- The scope only emits on destruction, after the wrapped serializer call has run

**Example (classic receive):**

```cpp
MICROMEGAS_NET_PROPERTY_SCOPE(Cmd.Property->GetFName(), Params.Bunch.GetPosBits());
```

**Example (Iris receive):**

```cpp
MICROMEGAS_NET_PROPERTY_SCOPE(MemberDebugDescriptors[MemberIt].DebugName->Name,
                              Context.GetBitStreamReader()->GetPosBits());
```

### MICROMEGAS_NET_SUSPEND_SCOPE

Zeroes out every `MICROMEGAS_NET_*` call inside its lifetime without touching depth counters. Safe to nest under an active live scope.

```cpp
MICROMEGAS_NET_SUSPEND_SCOPE()
```

**Parameters:** none.

**Semantics:**

- RAII; `Dispatch::NetSuspend()` on entry, `NetResume()` on exit
- Use for code paths that process packets/bunches but **shouldn't** contribute to attribution: demo recording, replay scrubbing, server-side simulation

**Example:**

```cpp
void UDemoNetDriver::ProcessRemoteFunction(...)
{
    MICROMEGAS_NET_SUSPEND_SCOPE();
    InternalProcessRemoteFunction(...);
}
```

### Verbosity Levels

Runtime verbosity is a 0–4 enum. Depth-based gating inside `NetTraceWriter`:

| Level | Name | Emits |
|-------|------|-------|
| 0 | `Off` | Nothing |
| 1 | `Packets` | Connection scopes only |
| 2 | `RootObjects` | + root object scopes (depth 0) |
| 3 | `Objects` | + nested object scopes (depth 1+) |
| 4 | `Properties` | + per-property leaf events, + RPC scopes |

**Default:** level 2 (`RootObjects`) — production setting.

Root RPC bits (`ObjectDepth == 0`) still contribute to `NetConnectionEndEvent.bit_size` at every verbosity ≥ `Packets`, even though `NetRPCBeginEvent` / `NetRPCEndEvent` records are only emitted at level 4.

**Snapshot invariant:** the writer captures `EffectiveVerbosity` at the outermost `BeginConnection` and uses that snapshot for every gating decision in the scope. CVar-driven changes take effect at the **next** outer scope, never mid-scope.

### Console & Command Line

- **CVar:** `telemetry.net.verbosity <0-4>` — sets runtime verbosity. Effective at the next outer connection scope.
- **Command-line flag:** `-MicromegasNetTrace=N` — sets initial verbosity at process start.

### Physical packet metrics

Two integer metrics are emitted via `MICROMEGAS_IMETRIC` from the instrumented engine code (see [Network Tracing § 3.1, § 3.2](network-tracing.md#31-incoming-packet-scope)):

- `net.packet_sent_bits` (unit `bits`) — `SendBuffer.GetNumBits()` in `FlushNet`
- `net.packet_received_bits` (unit `bits`) — `Reader.GetNumBits()` in `ReceivedPacket`

These are **wire bits** including packet headers, bunch headers, NetGUID exports, control bunches, and voice. Compare against `sum(NetConnectionEndEvent.bit_size)` for content-vs-wire reconciliation — the gap is framing overhead.

## Default Context API

The Default Context allows setting global properties that are automatically attached to all telemetry.

### Accessing the Default Context

```cpp
MicromegasTracing::DefaultContext* Ctx = 
    MicromegasTracing::Dispatch::GetDefaultContext();
```

### Set

Adds or updates a context property.

```cpp
void Set(FName Key, FName Value)
```

**Example:**
```cpp
if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
{
    Ctx->Set(FName("user_id"), FName(*UserId));
    Ctx->Set(FName("session_id"), FName(*SessionId));
    Ctx->Set(FName("map"), FName(*GetWorld()->GetMapName()));
}
```

### Unset

Removes a context property.

```cpp
void Unset(FName Key)
```

**Example:**
```cpp
Ctx->Unset(FName("temp_flag"));
```

### Clear

Removes all context properties.

```cpp
void Clear()
```

**Example:**
```cpp
// Clear context on logout
Ctx->Clear();
```

### Copy

Copies current context to a map.

```cpp
void Copy(TMap<FName, FName>& Out) const
```

**Example:**
```cpp
TMap<FName, FName> CurrentContext;
Ctx->Copy(CurrentContext);
// Examine or log current context
```

## Console Commands

Runtime control commands available in the Unreal console:

### telemetry.enable
Initializes the telemetry system if not already enabled.

```
telemetry.enable
```

### telemetry.flush
Forces immediate flush of all pending telemetry events.

```
telemetry.flush
```

### telemetry.spans.enable
Enables or disables span recording.

```
telemetry.spans.enable 1  // Enable spans
telemetry.spans.enable 0  // Disable spans
```

### telemetry.spans.all
Enables recording of all spans without sampling.

```
telemetry.spans.all 1  // Record all spans
telemetry.spans.all 0  // Use sampling
```

## Best Practices

### Performance

1. **Use sampling for high-frequency spans** - It's OK to keep spans enabled with reasonable sampling
2. **Use appropriate verbosity** - Lower verbosity for high-frequency metrics
3. **Batch operations** - Let the system batch; avoid frequent flushes
4. **Static strings** - Use TEXT() macro for string literals
5. **Limit context cardinality** - Context keys/values are never freed


### Error Handling

Always check for null pointers when using the context API:

```cpp
if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
{
    // Safe to use Ctx
    Ctx->Set(FName("key"), FName("value"));
}
```

### Thread Safety

All Micromegas APIs are thread-safe and can be called from any thread:

```cpp
// Safe from game thread
MICROMEGAS_LOG("Game", MicromegasTracing::LogLevel::Info, TEXT("Game thread"));

// Safe from render thread
MICROMEGAS_LOG("Render", MicromegasTracing::LogLevel::Info, TEXT("Render thread"));

// Safe from worker threads
ParallelFor(NumItems, [](int32 Index)
{
    MICROMEGAS_IMETRIC("Worker", MicromegasTracing::Verbosity::High,
                       TEXT("ItemProcessed"), TEXT("count"), 1);
});
```

## Integration Examples

### With Gameplay Abilities

```cpp
void UMyGameplayAbility::ActivateAbility(...)
{
    MICROMEGAS_SPAN_NAME("Abilities", GetFName());
    MICROMEGAS_LOG("Abilities", MicromegasTracing::LogLevel::Info,
                   FString::Printf(TEXT("Ability %s activated"), *GetName()));
    
    Super::ActivateAbility(...);
}
```

### With Animation

```cpp
void UAnimInstance::NativeUpdateAnimation(float DeltaSeconds)
{
    MICROMEGAS_SPAN_FUNCTION("Animation");
    MICROMEGAS_FMETRIC("Animation", MicromegasTracing::Verbosity::High,
                       TEXT("UpdateTime"), TEXT("ms"), DeltaSeconds * 1000);
    
    Super::NativeUpdateAnimation(DeltaSeconds);
}
```

### With Networking

```cpp
void AMyPlayerController::ClientRPC_Implementation()
{
    MICROMEGAS_LOG("Network", MicromegasTracing::LogLevel::Debug,
                   TEXT("RPC received"));
    MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med,
                       TEXT("RPCCount"), TEXT("count"), 1);
}
```