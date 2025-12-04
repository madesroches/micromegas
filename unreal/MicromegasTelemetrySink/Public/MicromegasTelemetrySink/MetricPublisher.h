#pragma once
//
//  MicromegasTelemetrySink/MetricPublisher.h
//
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"
#include "Engine/EngineBaseTypes.h"
#include "Runtime/Engine/Public/Scalability.h"
#include "Engine/World.h"

class UWorld;
class ULevel;

class MICROMEGASTELEMETRYSINK_API MetricPublisher
{
public:
	MetricPublisher();
	~MetricPublisher();

private:
	void Tick();
	void UpdateMapInContext(UWorld* World);

	void OnWorldInit(UWorld* /*World*/, const UWorld::InitializationValues /*IVS*/);
	void OnWorldTornDown(UWorld* World);
	static void EmitScalabilityMetrics(const Scalability::FQualityLevels& NewLevels);
	static void EmitVSyncStatus(IConsoleVariable* CVar);

	FName CurrentWorldName;
};

typedef TSharedPtr<MetricPublisher, ESPMode::ThreadSafe> SharedMetricPublisher;
