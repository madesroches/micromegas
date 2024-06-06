#pragma once
//
//  MicromegasTelemetrySink/MetricPublisher.h
//
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"

class MICROMEGASTELEMETRYSINK_API MetricPublisher
{
public:
	MetricPublisher();
	~MetricPublisher();

private:
	void Tick();
};

typedef TSharedPtr<MetricPublisher, ESPMode::ThreadSafe> SharedMetricPublisher;
