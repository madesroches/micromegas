//
//  MicromegasTelemetrySink/MetricPublisher.cpp
//
#include "MicromegasTelemetrySink/MetricPublisher.h"
#include "HAL/PlatformTime.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/App.h"
#include "Misc/CoreDelegates.h"

MetricPublisher::MetricPublisher()
{
	FCoreDelegates::OnBeginFrame.AddRaw(this, &MetricPublisher::Tick);
}

MetricPublisher::~MetricPublisher()
{
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
}

void MetricPublisher::Tick()
{
	MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("DeltaTime"), TEXT("seconds"), FApp::GetDeltaTime());

	FPlatformMemoryStats MemStats = FPlatformMemory::GetStats();
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedPhysical"), TEXT("bytes"), MemStats.UsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedPhysical"), TEXT("bytes"), MemStats.PeakUsedPhysical);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("UsedVirtual"), TEXT("bytes"), MemStats.UsedVirtual);
	MICROMEGAS_IMETRIC("Memory", MicromegasTracing::Verbosity::Med, TEXT("PeakUsedVirtual"), TEXT("bytes"), MemStats.PeakUsedVirtual);
}
