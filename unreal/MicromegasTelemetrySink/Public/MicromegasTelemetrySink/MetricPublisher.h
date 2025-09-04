#pragma once
//
//  MicromegasTelemetrySink/MetricPublisher.h
//
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"
#include "Engine/EngineBaseTypes.h"
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
	void EmitScalabilityMetrics(const Scalability::FQualityLevels& QualityLevels);

	FName CurrentWorldName;
};

typedef TSharedPtr<MetricPublisher, ESPMode::ThreadSafe> SharedMetricPublisher;
