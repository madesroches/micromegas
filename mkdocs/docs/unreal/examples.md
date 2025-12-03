# Examples

Practical examples of instrumenting common Unreal Engine scenarios with Micromegas.

## Basic Setup

### Game Instance Initialization

Complete setup with authentication and default context:

```cpp
// MyGameInstance.h
UCLASS()
class MYGAME_API UMyGameInstance : public UGameInstance
{
    GENERATED_BODY()
    
public:
    virtual void Init() override;
    virtual void Shutdown() override;
    
private:
    void SetupTelemetryContext();
};

// MyGameInstance.cpp
#include "MyGameInstance.h"
#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "MicromegasTelemetrySink/ApiKeyAuthenticator.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/DefaultContext.h"

void UMyGameInstance::Init()
{
    Super::Init();

    // Get API key from config (store securely, not hardcoded!)
    FString ApiKey = GetDefault<UGameSettings>()->TelemetryApiKey;

    // Initialize telemetry with API key authentication
    auto Auth = MakeShared<FApiKeyAuthenticator>(ApiKey);
    IMicromegasTelemetrySinkModule::LoadModuleChecked().InitTelemetry(
        TEXT("https://telemetry.example.com:9000"),
        Auth
    );

    SetupTelemetryContext();

    MICROMEGAS_LOG("Game", MicromegasTracing::LogLevel::Info,
                   TEXT("Game instance initialized"));
}

void UMyGameInstance::SetupTelemetryContext()
{
    if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
    {
        // Session info
        Ctx->Set(FName("session_id"), FName(*FGuid::NewGuid().ToString()));
        Ctx->Set(FName("timestamp"), FName(*FDateTime::UtcNow().ToString()));
        
        // Build info
        Ctx->Set(FName("build_version"), FName(TEXT(GAME_VERSION)));
        Ctx->Set(FName("build_config"), FName(TEXT(STRINGIFY(UE_BUILD_CONFIGURATION))));
        
        // Platform info
        Ctx->Set(FName("platform"), FName(*UGameplayStatics::GetPlatformName()));
        Ctx->Set(FName("cpu"), FName(*FPlatformMisc::GetCPUBrand()));
        Ctx->Set(FName("gpu"), FName(*GRHIAdapterName));
        
        // Player info (if available)
        if (ULocalPlayer* LocalPlayer = GetFirstGamePlayer())
        {
            Ctx->Set(FName("player_id"), FName(*LocalPlayer->GetPreferredUniqueNetId().ToString()));
        }
    }
}

void UMyGameInstance::Shutdown()
{
    MICROMEGAS_LOG("Game", MicromegasTracing::LogLevel::Info, 
                   TEXT("Game instance shutting down"));
    
    // Force flush before shutdown
    MicromegasTracing::Dispatch::FlushLogStream();
    MicromegasTracing::Dispatch::FlushMetricStream();
    MicromegasTracing::Dispatch::FlushCurrentThreadStream();
    
    Super::Shutdown();
}
```

## Game Loop Instrumentation

### Game Mode with Performance Metrics

```cpp
// MyGameMode.cpp
void AMyGameMode::Tick(float DeltaSeconds)
{
    MICROMEGAS_SPAN_FUNCTION("Game.GameMode");
    
    Super::Tick(DeltaSeconds);
    
    // Frame metrics
    MICROMEGAS_FMETRIC("Performance", MicromegasTracing::Verbosity::Med,
                       TEXT("FrameTime"), TEXT("ms"), DeltaSeconds * 1000.0f);
    
    MICROMEGAS_FMETRIC("Performance", MicromegasTracing::Verbosity::Med,
                       TEXT("FPS"), TEXT("fps"), 1.0f / DeltaSeconds);
    
    // Game state metrics
    MICROMEGAS_IMETRIC("Game", MicromegasTracing::Verbosity::Low,
                       TEXT("PlayerCount"), TEXT("count"), 
                       GetNumPlayers());
    
    MICROMEGAS_IMETRIC("Game", MicromegasTracing::Verbosity::Low,
                       TEXT("AICount"), TEXT("count"),
                       GetWorld()->GetNumPawns() - GetNumPlayers());
    
    // Memory metrics (every 60 frames)
    static int32 FrameCounter = 0;
    if (++FrameCounter % 60 == 0)
    {
        FPlatformMemoryStats MemStats = FPlatformMemory::GetStats();
        MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Low,
                           TEXT("WorkingSetSize"), TEXT("bytes"),
                           MemStats.UsedPhysical);
    }
}

void AMyGameMode::HandleMatchIsWaitingToStart()
{
    MICROMEGAS_LOG("Game.Match", MicromegasTracing::LogLevel::Info,
                   TEXT("Match waiting to start"));
    Super::HandleMatchIsWaitingToStart();
}

void AMyGameMode::HandleMatchHasStarted()
{
    MICROMEGAS_SPAN_FUNCTION("Game.Match");
    MICROMEGAS_LOG("Game.Match", MicromegasTracing::LogLevel::Info,
                   FString::Printf(TEXT("Match started on map: %s"), 
                   *GetWorld()->GetMapName()));
    
    // Update context with match info
    if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
    {
        Ctx->Set(FName("match_id"), FName(*FGuid::NewGuid().ToString()));
        Ctx->Set(FName("map"), FName(*GetWorld()->GetMapName()));
        Ctx->Set(FName("game_mode"), FName(*GetClass()->GetName()));
    }
    
    Super::HandleMatchHasStarted();
}
```

## Actor and Component Lifecycle

### Instrumented Actor

```cpp
// MyActor.cpp
#include "MicromegasTracing/Macros.h"

void AMyActor::BeginPlay()
{
    MICROMEGAS_SPAN_UOBJECT("Actor.Lifecycle", this);
    
    Super::BeginPlay();
    
    MICROMEGAS_LOG("Actor", MicromegasTracing::LogLevel::Debug,
                   FString::Printf(TEXT("%s spawned at %s"), 
                   *GetName(), *GetActorLocation().ToString()));
    
    // Track actor spawns by class
    MICROMEGAS_IMETRIC("Actor", MicromegasTracing::Verbosity::Med,
                       *FString::Printf(TEXT("Spawned.%s"), *GetClass()->GetName()),
                       TEXT("count"), 1);
}

void AMyActor::Tick(float DeltaTime)
{
    // Only trace tick for important actors
    if (bIsImportant)
    {
        MICROMEGAS_SPAN_UOBJECT("Actor.Tick", this);
        Super::Tick(DeltaTime);
        
        // Actor-specific logic...
    }
    else
    {
        Super::Tick(DeltaTime);
    }
}

void AMyActor::EndPlay(const EEndPlayReason::Type EndPlayReason)
{
    MICROMEGAS_LOG("Actor", MicromegasTracing::LogLevel::Debug,
                   FString::Printf(TEXT("%s destroyed: %s"), 
                   *GetName(), 
                   *UEnum::GetValueAsString(EndPlayReason)));
    
    Super::EndPlay(EndPlayReason);
}

float AMyActor::TakeDamage(float DamageAmount, FDamageEvent const& DamageEvent,
                           AController* EventInstigator, AActor* DamageCauser)
{
    float ActualDamage = Super::TakeDamage(DamageAmount, DamageEvent, 
                                           EventInstigator, DamageCauser);
    
    MICROMEGAS_LOG("Combat", MicromegasTracing::LogLevel::Info,
                   FString::Printf(TEXT("%s took %.1f damage from %s"), 
                   *GetName(), ActualDamage,
                   DamageCauser ? *DamageCauser->GetName() : TEXT("Unknown")));
    
    MICROMEGAS_FMETRIC("Combat", MicromegasTracing::Verbosity::High,
                       TEXT("DamageDealt"), TEXT("points"), ActualDamage);
    
    return ActualDamage;
}
```

## Player Controller

### Player Actions and Input

```cpp
// MyPlayerController.cpp
void AMyPlayerController::BeginPlay()
{
    Super::BeginPlay();
    
    if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
    {
        // Set player-specific context
        Ctx->Set(FName("player_name"), FName(*PlayerState->GetPlayerName()));
        Ctx->Set(FName("player_id"), FName(*GetUniqueID().ToString()));
    }
    
    MICROMEGAS_LOG("Player", MicromegasTracing::LogLevel::Info,
                   FString::Printf(TEXT("Player %s joined"), 
                   *PlayerState->GetPlayerName()));
}

void AMyPlayerController::SetupInputComponent()
{
    Super::SetupInputComponent();
    
    InputComponent->BindAction("Fire", IE_Pressed, this, &AMyPlayerController::OnFire);
    InputComponent->BindAction("Jump", IE_Pressed, this, &AMyPlayerController::OnJump);
    InputComponent->BindAction("Interact", IE_Pressed, this, &AMyPlayerController::OnInteract);
}

void AMyPlayerController::OnFire()
{
    MICROMEGAS_LOG("Player.Input", MicromegasTracing::LogLevel::Trace,
                   TEXT("Fire action"));
    MICROMEGAS_IMETRIC("Player.Actions", MicromegasTracing::Verbosity::High,
                       TEXT("Fire"), TEXT("count"), 1);
    
    // Fire logic...
}

void AMyPlayerController::OnJump()
{
    MICROMEGAS_IMETRIC("Player.Actions", MicromegasTracing::Verbosity::High,
                       TEXT("Jump"), TEXT("count"), 1);
    // Jump logic...
}

void AMyPlayerController::OnInteract()
{
    MICROMEGAS_SPAN_SCOPE("Player.Interaction", "Interact");
    
    FHitResult HitResult;
    if (GetHitResultUnderCursor(ECC_Pawn, false, HitResult))
    {
        if (AActor* HitActor = HitResult.GetActor())
        {
            MICROMEGAS_LOG("Player.Interaction", MicromegasTracing::LogLevel::Info,
                           FString::Printf(TEXT("Interacting with %s"), 
                           *HitActor->GetName()));
            
            // Interaction logic...
        }
    }
}
```

## Network Replication

### Network Metrics and Events

```cpp
// MyGameState.cpp
void AMyGameState::Tick(float DeltaSeconds)
{
    Super::Tick(DeltaSeconds);
    
    // Network metrics (every second)
    TimeSinceLastNetworkUpdate += DeltaSeconds;
    if (TimeSinceLastNetworkUpdate >= 1.0f)
    {
        TimeSinceLastNetworkUpdate = 0.0f;
        
        if (UNetDriver* NetDriver = GetWorld()->GetNetDriver())
        {
            MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med,
                               TEXT("ClientConnections"), TEXT("count"),
                               NetDriver->ClientConnections.Num());
            
            MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med,
                               TEXT("TotalNetObjects"), TEXT("count"),
                               NetDriver->GetNetworkObjectList().GetObjects().Num());
            
            // Bandwidth metrics
            MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med,
                               TEXT("InBytes"), TEXT("bytes"),
                               NetDriver->InBytes);
            
            MICROMEGAS_IMETRIC("Network", MicromegasTracing::Verbosity::Med,
                               TEXT("OutBytes"), TEXT("bytes"),
                               NetDriver->OutBytes);
        }
    }
}

// RPC tracking
void AMyGameState::ServerRPC_Implementation(const FString& Data)
{
    MICROMEGAS_SPAN_SCOPE("Network.RPC", "ServerRPC");
    MICROMEGAS_IMETRIC("Network.RPC", MicromegasTracing::Verbosity::High,
                       TEXT("ServerCalls"), TEXT("count"), 1);
    
    // Process RPC...
}

void AMyGameState::ClientRPC_Implementation(const FString& Data)
{
    MICROMEGAS_IMETRIC("Network.RPC", MicromegasTracing::Verbosity::High,
                       TEXT("ClientCalls"), TEXT("count"), 1);
    
    // Process RPC...
}

void AMyGameState::OnRep_ReplicatedProperty()
{
    MICROMEGAS_IMETRIC("Network.Replication", MicromegasTracing::Verbosity::High,
                       TEXT("PropertyUpdates"), TEXT("count"), 1);
}
```

## Asset Loading and Streaming

### Content Loading Instrumentation

```cpp
// MyAssetManager.cpp
void UMyAssetManager::LoadAssetAsync(const FString& AssetPath)
{
    MICROMEGAS_SPAN_NAME("Content.AsyncLoad", *AssetPath);
    
    MICROMEGAS_LOG("Content", MicromegasTracing::LogLevel::Debug,
                   FString::Printf(TEXT("Loading asset: %s"), *AssetPath));
    
    FStreamableManager& Streamable = UAssetManager::GetStreamableManager();
    TSharedPtr<FStreamableHandle> Handle = Streamable.RequestAsyncLoad(
        FSoftObjectPath(AssetPath),
        FStreamableDelegate::CreateLambda([AssetPath]()
        {
            MICROMEGAS_LOG("Content", MicromegasTracing::LogLevel::Debug,
                           FString::Printf(TEXT("Asset loaded: %s"), *AssetPath));
            
            MICROMEGAS_IMETRIC("Content", MicromegasTracing::Verbosity::Med,
                               TEXT("AssetsLoaded"), TEXT("count"), 1);
        })
    );
}

void UMyAssetManager::OnLevelStreamingComplete(ULevelStreaming* StreamedLevel)
{
    if (StreamedLevel && StreamedLevel->GetLoadedLevel())
    {
        int64 SizeBytes = StreamedLevel->GetLoadedLevel()->GetOutermost()->GetFileSize();
        
        MICROMEGAS_LOG("Content.Streaming", MicromegasTracing::LogLevel::Info,
                       FString::Printf(TEXT("Level streamed: %s (%.2f MB)"), 
                       *StreamedLevel->GetWorldAssetPackageFName().ToString(),
                       SizeBytes / (1024.0f * 1024.0f)));
        
        MICROMEGAS_IMETRIC("Content.Streaming", MicromegasTracing::Verbosity::Low,
                           TEXT("LevelSize"), TEXT("bytes"), SizeBytes);
    }
}
```

## AI and Behavior Trees

### AI Controller Instrumentation

```cpp
// MyAIController.cpp
void AMyAIController::RunBehaviorTree(UBehaviorTree* BTAsset)
{
    MICROMEGAS_SPAN_SCOPE("AI.BehaviorTree", "RunTree");
    
    MICROMEGAS_LOG("AI", MicromegasTracing::LogLevel::Debug,
                   FString::Printf(TEXT("Starting behavior tree: %s"), 
                   *BTAsset->GetName()));
    
    return Super::RunBehaviorTree(BTAsset);
}

void AMyAIController::OnMoveCompleted(FAIRequestID RequestID, EPathFollowingResult::Type Result)
{
    MICROMEGAS_LOG("AI.Movement", MicromegasTracing::LogLevel::Trace,
                   FString::Printf(TEXT("AI move completed: %s"), 
                   *UEnum::GetValueAsString(Result)));
    
    MICROMEGAS_IMETRIC("AI.Movement", MicromegasTracing::Verbosity::High,
                       TEXT("MovesCompleted"), TEXT("count"), 1);
    
    Super::OnMoveCompleted(RequestID, Result);
}

// BTTask instrumentation
EBTNodeResult::Type UMyBTTask::ExecuteTask(UBehaviorTreeComponent& OwnerComp, uint8* NodeMemory)
{
    MICROMEGAS_SPAN_SCOPE("AI.BTTask", GetNodeName());
    
    EBTNodeResult::Type Result = Super::ExecuteTask(OwnerComp, NodeMemory);
    
    MICROMEGAS_LOG("AI.BehaviorTree", MicromegasTracing::LogLevel::Trace,
                   FString::Printf(TEXT("Task %s: %s"), 
                   *GetNodeName(),
                   *UEnum::GetValueAsString(Result)));
    
    return Result;
}
```

## Profiling Critical Paths

### Render Thread Instrumentation

```cpp
// MySceneProxy.cpp
void FMySceneProxy::GetDynamicMeshElements(...)
{
    MICROMEGAS_SPAN_SCOPE("Render.SceneProxy", "GetDynamicMeshElements");
    
    // Expensive rendering operations
    MICROMEGAS_IMETRIC("Render", MicromegasTracing::Verbosity::High,
                       TEXT("DynamicElements"), TEXT("count"), Elements.Num());
}
```

### Physics Simulation

```cpp
// MyPhysicsActor.cpp
void AMyPhysicsActor::SimulatePhysics(float DeltaTime)
{
    MICROMEGAS_SPAN_FUNCTION("Physics.Simulation");
    
    double StartTime = FPlatformTime::Seconds();
    
    // Run physics simulation
    RunComplexPhysicsSimulation(DeltaTime);
    
    double SimTime = (FPlatformTime::Seconds() - StartTime) * 1000.0;
    MICROMEGAS_FMETRIC("Physics", MicromegasTracing::Verbosity::Med,
                       TEXT("SimulationTime"), TEXT("ms"), SimTime);
    
    if (SimTime > 16.0) // Longer than a frame
    {
        MICROMEGAS_LOG("Physics", MicromegasTracing::LogLevel::Warn,
                       FString::Printf(TEXT("Physics simulation took %.2fms"), SimTime));
    }
}
```

## Error Handling and Debugging

### Comprehensive Error Logging

```cpp
void UMyGameSubsystem::HandleError(const FString& ErrorContext, const FString& ErrorMessage)
{
    // Log the error
    MICROMEGAS_LOG("Error", MicromegasTracing::LogLevel::Error,
                   FString::Printf(TEXT("[%s] %s"), *ErrorContext, *ErrorMessage));
    
    // Track error metrics
    MICROMEGAS_IMETRIC("Errors", MicromegasTracing::Verbosity::Low,
                       *FString::Printf(TEXT("Error.%s"), *ErrorContext),
                       TEXT("count"), 1);
    
    // Add error to context for correlation
    if (auto* Ctx = MicromegasTracing::Dispatch::GetDefaultContext())
    {
        Ctx->Set(FName("last_error"), FName(*ErrorMessage));
        Ctx->Set(FName("error_time"), FName(*FDateTime::UtcNow().ToString()));
    }
    
    // Force flush for critical errors
    if (IsCriticalError(ErrorContext))
    {
        MicromegasTracing::Dispatch::FlushLogStream();
        MicromegasTracing::Dispatch::FlushMetricStream();
        MicromegasTracing::Dispatch::FlushCurrentThreadStream();
    }
}

// Assertion handler
void CheckGameState(bool bCondition, const FString& Message)
{
    if (!bCondition)
    {
        MICROMEGAS_LOG("Assert", MicromegasTracing::LogLevel::Fatal,
                       FString::Printf(TEXT("Assertion failed: %s"), *Message));
        
        // Flush before potential crash
        MicromegasTracing::Dispatch::FlushLogStream();
        MicromegasTracing::Dispatch::FlushMetricStream();
        MicromegasTracing::Dispatch::FlushCurrentThreadStream();
        
        check(false);
    }
}
```

