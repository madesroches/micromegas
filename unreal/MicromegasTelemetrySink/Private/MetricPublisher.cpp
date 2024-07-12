//
//  MicromegasTelemetrySink/MetricPublisher.cpp
//
#include "MicromegasTelemetrySink/MetricPublisher.h"

#include "HAL/PlatformTime.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/App.h"
#include "Misc/CoreDelegates.h"
#include "Engine/World.h"

MetricPublisher::MetricPublisher()
{
	FCoreUObjectDelegates::PostLoadMapWithWorld.AddRaw(this, &MetricPublisher::OnWorldCreated);
	FWorldDelegates::OnWorldBeginTearDown.AddRaw(this, &MetricPublisher::OnWorldTornDown);
	FCoreDelegates::OnBeginFrame.AddRaw(this, &MetricPublisher::Tick);
}

MetricPublisher::~MetricPublisher()
{
	FCoreUObjectDelegates::PostLoadMapWithWorld.RemoveAll(this);
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
	FWorldDelegates::OnWorldBeginTearDown.RemoveAll(this);
}

void MetricPublisher::Tick()
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("DeltaTime"), TEXT("seconds"), FApp::GetDeltaTime());

	FPlatformMemoryStats MemStats = FPlatformMemory::GetStats();
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedPhysical"), TEXT("bytes"), MemStats.UsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedPhysical"), TEXT("bytes"), MemStats.PeakUsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedVirtual"), TEXT("bytes"), MemStats.UsedVirtual);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedVirtual"), TEXT("bytes"), MemStats.PeakUsedVirtual);
}

void MetricPublisher::OnWorldCreated(UWorld* World)
{
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("New world created: Map=%s"), *World->GetMapName());
}

void MetricPublisher::OnWorldTornDown(UWorld* World)
{
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("World torn down: Map=%s"), *World->GetMapName());
}
