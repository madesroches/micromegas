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
#include "UnrealClient.h"

MetricPublisher::MetricPublisher()
{
	FWorldDelegates::OnPreWorldInitialization.AddRaw(this, &MetricPublisher::OnWorldInit);
	FWorldDelegates::OnWorldBeginTearDown.AddRaw(this, &MetricPublisher::OnWorldTornDown);
	FCoreDelegates::OnBeginFrame.AddRaw(this, &MetricPublisher::Tick);
	Scalability::OnScalabilitySettingsChanged.AddRaw(this, &MetricPublisher::EmitScalabilityMetrics);
}

MetricPublisher::~MetricPublisher()
{
	FWorldDelegates::OnPreWorldInitialization.RemoveAll(this);
	FWorldDelegates::OnWorldBeginTearDown.RemoveAll(this);
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
	Scalability::OnScalabilitySettingsChanged.RemoveAll(this);
}

void MetricPublisher::EmitScalabilityMetrics(const Scalability::FQualityLevels& NewLevels)
{
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("LandscapeQuality"), TEXT("none"), NewLevels.LandscapeQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("EffectsQuality"), TEXT("none"), NewLevels.EffectsQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("FoliageQuality"), TEXT("none"), NewLevels.FoliageQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("ReflectionQuality"), TEXT("none"), NewLevels.ReflectionQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("ShadingQuality"), TEXT("none"), NewLevels.ShadingQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("ShadowQuality"), TEXT("none"), NewLevels.ShadowQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("TextureQuality"), TEXT("none"), NewLevels.TextureQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("AntiAliasingQuality"), TEXT("none"), NewLevels.AntiAliasingQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("GlobalIlluminationQuality"), TEXT("none"), NewLevels.GlobalIlluminationQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("PostProcessQuality"), TEXT("none"), NewLevels.PostProcessQuality);
	MICROMEGAS_IMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("ViewDistanceQuality"), TEXT("none"), NewLevels.ViewDistanceQuality);
	MICROMEGAS_FMETRIC("Scalability", MicromegasTracing::Verbosity::Min, TEXT("ResolutionQuality"), TEXT("none"), NewLevels.ResolutionQuality);
}

void MetricPublisher::OnWorldInit(UWorld* World, const UWorld::InitializationValues /*IVS*/)
{
	UpdateMapInContext(World);
}

void MetricPublisher::OnWorldTornDown(UWorld* World)
{
	FName WorldName = World->GetOutermost()->GetFName();
	if (CurrentWorldName == WorldName && GWorld != nullptr)
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
	if (CurrentWorldName != WorldName)
	{
		CurrentWorldName = WorldName;
		static const FName MapProperty("map");
		Ctx->Set(MapProperty, CurrentWorldName);
	}
}

void MetricPublisher::Tick()
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("DeltaTime"), TEXT("seconds"), FApp::GetCurrentTime() - FApp::GetLastTime());
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("GameThreadTime"), TEXT("seconds"), FPlatformTime::ToSeconds64(GGameThreadTime));
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("RenderThreadTime"), TEXT("seconds"), FPlatformTime::ToSeconds64(GRenderThreadTime));
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("RHIThreadTime"), TEXT("seconds"), FPlatformTime::ToSeconds64(GRHIThreadTime));
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("InputLatencyTime"), TEXT("seconds"), FPlatformTime::ToSeconds64(GInputLatencyTime));
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("GPUTime"), TEXT("seconds"), FPlatformTime::ToSeconds64(RHIGetGPUFrameCycles(0)));

	FPlatformMemoryStats MemStats = FPlatformMemory::GetStats();
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedPhysical"), TEXT("bytes"), MemStats.UsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedPhysical"), TEXT("bytes"), MemStats.PeakUsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedVirtual"), TEXT("bytes"), MemStats.UsedVirtual);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedVirtual"), TEXT("bytes"), MemStats.PeakUsedVirtual);
}
