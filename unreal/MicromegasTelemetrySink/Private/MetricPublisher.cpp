//
//  MicromegasTelemetrySink/MetricPublisher.cpp
//
#include "MicromegasTelemetrySink/MetricPublisher.h"

#include "Engine/World.h"
#include "HAL/PlatformTime.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/DefaultContext.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/App.h"
#include "Misc/CoreDelegates.h"
#include "UObject/Package.h"

MetricPublisher::MetricPublisher()
{
	FWorldDelegates::OnPreWorldInitialization.AddRaw(this, &MetricPublisher::OnWorldInit);
	FWorldDelegates::OnWorldBeginTearDown.AddRaw(this, &MetricPublisher::OnWorldTornDown);
	FCoreDelegates::OnBeginFrame.AddRaw(this, &MetricPublisher::Tick);
}

MetricPublisher::~MetricPublisher()
{
	FWorldDelegates::OnPreWorldInitialization.RemoveAll(this);
	FWorldDelegates::OnWorldBeginTearDown.RemoveAll(this);
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
}

void MetricPublisher::OnWorldInit(UWorld* World, const UWorld::InitializationValues /*IVS*/)
{
	UpdateMapInContext(World);
}

void MetricPublisher::OnWorldTornDown(UWorld* World)
{
	FName WorldName = World->GetOutermost()->GetFName();
	if (CurrentWorld == WorldName && GWorld != nullptr)
	{
		UpdateMapInContext(GWorld);
	}
}

void MetricPublisher::UpdateMapInContext(UWorld* World)
{
	MicromegasTracing::DefaultContext* Ctx = MicromegasTracing::Dispatch::GetDefaultContext();
	if (!Ctx)
	{
		return;
	}

	FName WorldName = World->GetOutermost()->GetFName();
	if (CurrentWorld != WorldName)
	{
		CurrentWorld = WorldName;
		static const FName MapProperty("map");
		Ctx->Set(MapProperty, CurrentWorld);
	}
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
