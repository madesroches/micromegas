#pragma once
//
//  MicromegasTelemetrySink/MetricPublisher.h
//
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"

class UWorld;

class MICROMEGASTELEMETRYSINK_API MetricPublisher
{
public:
	MetricPublisher();
	~MetricPublisher();

private:
	void Tick();

	void OnWorldCreated(UWorld* World);
	void OnWorldTornDown(UWorld* World);
};

typedef TSharedPtr<MetricPublisher, ESPMode::ThreadSafe> SharedMetricPublisher;
