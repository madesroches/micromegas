# Installation Guide

This guide walks through installing and configuring the Micromegas Unreal Engine integration.

## Prerequisites

- Unreal Engine 4.27+ or 5.0+
- Visual Studio 2019 or 2022 (Windows)
- Xcode (Mac)
- A running Micromegas ingestion server

## Standard Installation

### Step 1: Copy the Modules

1. Copy the Core module extension to Unreal's Core module:
   ```
   micromegas/unreal/MicromegasTracing/Public/MicromegasTracing → 
   YourUnrealEngine/Engine/Source/Runtime/Core/Public/MicromegasTracing
   
   micromegas/unreal/MicromegasTracing/Private →
   YourUnrealEngine/Engine/Source/Runtime/Core/Private/MicromegasTracing
   ```

2. Copy the plugin to your project:
   ```
   micromegas/unreal/MicromegasTelemetrySink → YourProject/Plugins/MicromegasTelemetrySink
   ```

### Step 2: Configure Build Dependencies

Since MicromegasTracing is now part of the Core module, you only need to add the plugin:

```csharp
// YourGame.Build.cs
public class YourGame : ModuleRules
{
    public YourGame(ReadOnlyTargetRules Target) : base(Target)
    {
        PublicDependencyModuleNames.AddRange(new string[] { 
            "Core",  // MicromegasTracing is now included in Core
            "CoreUObject", 
            "Engine"
        });
        
        PrivateDependencyModuleNames.AddRange(new string[] {
            "MicromegasTelemetrySink"  // Add this plugin
        });
    }
}
```

### Step 3: Enable the Plugin

Either:

- **Via Editor**: Go to Edit → Plugins → Search for "MicromegasTelemetrySink" → Enable
- **Via .uproject**: Add to the Plugins section:
  ```json
  {
    "Name": "MicromegasTelemetrySink",
    "Enabled": true
  }
  ```

### Step 4: Build the Project

Regenerate project files and build:

1. Right-click your `.uproject` file → "Generate Visual Studio project files"
2. Open the solution in Visual Studio/Xcode
3. Build the project

## Development Setup (Windows)

For active development on Micromegas while testing in Unreal, use hard links to avoid copying files:

### Step 1: Set Environment Variables

```batch
set MICROMEGAS_UNREAL_ROOT_DIR=C:\Program Files\Epic Games\UE_5.3
set MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR=C:\YourProject\Plugins
```

### Step 2: Run the Hard Link Script

```batch
cd micromegas/build
python unreal_hard_link_windows.py
```

This creates hard links that:

- Link MicromegasTracing into Unreal Engine's Core module
- Link MicromegasTelemetrySink plugin to your project  
- Allow you to edit Micromegas source and see changes immediately in Unreal

## Initial Configuration

### Basic Setup

In your `GameInstance` or `GameMode` class:

```cpp
// YourGameInstance.h
#pragma once
#include "Engine/GameInstance.h"
#include "YourGameInstance.generated.h"

UCLASS()
class YOURGAME_API UYourGameInstance : public UGameInstance
{
    GENERATED_BODY()
    
public:
    virtual void Init() override;
};

// YourGameInstance.cpp
#include "YourGameInstance.h"
#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/DefaultContext.h"

void UYourGameInstance::Init()
{
    Super::Init();
    
    // Initialize telemetry
    FString ServerUrl = TEXT("https://telemetry.yourcompany.com:9000");
    
    // Create authentication provider (implement your auth logic)
    auto AuthProvider = MakeShared<FMyTelemetryAuthenticator>();
    
    // Initialize the sink
    IMicromegasTelemetrySinkModule::LoadModuleChecked().InitTelemetry(
        ServerUrl, 
        AuthProvider
    );
    
    // Set default context properties
    if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
    {
        Ctx->Set(FName("build_version"), FName(TEXT("1.0.0")));
        Ctx->Set(FName("platform"), FName(*UGameplayStatics::GetPlatformName()));
        Ctx->Set(FName("session_id"), FName(*FGuid::NewGuid().ToString()));
    }
    
    UE_LOG(LogTemp, Log, TEXT("Telemetry initialized"));
}
```

### Authentication Provider

Implement the authentication interface:

```cpp
// MyTelemetryAuthenticator.h
#pragma once
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "Interfaces/IHttpRequest.h"

class FMyTelemetryAuthenticator : public ITelemetryAuthenticator
{
public:
    virtual ~FMyTelemetryAuthenticator() = default;
    
    virtual void Init(const MicromegasTracing::EventSinkPtr& InSink) override
    {
        // Initialize authenticator if needed
    }
    
    virtual bool IsReady() override
    {
        // Return true when authentication is ready
        return true;
    }
    
    virtual bool Sign(IHttpRequest& Request) override
    {
        // Add authentication to the HTTP request
        Request.SetHeader(TEXT("Authorization"), TEXT("Bearer your-api-key-here"));
        return true;
    }
};
```

## Configuration Options

### Compile-Time Settings

In `MicromegasTelemetrySinkModule.cpp`:

```cpp
// Enable telemetry on startup (default: 1)
#define MICROMEGAS_ENABLE_TELEMETRY_ON_START 1

// Enable crash reporting on Windows (default: 1 on Windows, 0 elsewhere)
#define MICROMEGAS_CRASH_REPORTING 1
```

### Runtime Console Commands

Available console commands for runtime control:

- `telemetry.enable` - Initialize the telemetry system
- `telemetry.flush` - Force flush all pending events
- `telemetry.spans.enable 1` - Enable span recording (disabled by default)
- `telemetry.spans.enable 0` - Disable span recording
- `telemetry.spans.all 1` - Record all spans without sampling

## Verifying Installation

### Test Basic Logging

Add to any Actor or GameMode:

```cpp
#include "MicromegasTracing/Macros.h"

void ATestActor::BeginPlay()
{
    Super::BeginPlay();
    
    // This should appear in your telemetry
    MICROMEGAS_LOG("Test", MicromegasTracing::LogLevel::Info, 
                   TEXT("Micromegas telemetry is working!"));
    
    // This metric should be recorded
    MICROMEGAS_IMETRIC("Test", MicromegasTracing::Verbosity::Med, 
                       TEXT("TestCounter"), TEXT("count"), 1);
}
```

### Check Server Connection

1. Run your game in the editor
2. Open the console (` key)
3. Type: `telemetry.flush`
4. Check your ingestion server logs for incoming requests
5. Query your data using the Python client or CLI tools

## Platform-Specific Notes

### Windows
- Crash reporting is enabled by default
- Requires debug symbols (`.pdb` files) for meaningful stack traces
- Windows Defender may flag the first network connection - add an exception if needed

### Linux
- Ensure your ingestion server is accessible from the game server
- Check firewall rules for port 9000 (or your configured port)

### Mac
- Code signing may be required for shipping builds
- Network permissions needed for telemetry upload

### Consoles
- Special network configuration required
- Contact your platform representative for network policy compliance



## Next Steps

- [Instrumentation API](instrumentation-api.md) - Learn about logging, metrics, and spans
- [Examples](examples.md) - See common usage patterns