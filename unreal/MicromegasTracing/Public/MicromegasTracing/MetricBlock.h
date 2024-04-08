#pragma once
//
//  MicromegasTracing/MetricBlock.h
//
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/MetricEvents.h"

namespace MicromegasTracing
{
	typedef HeterogeneousQueue<
		IntegerMetricEvent,
		FloatMetricEvent>
		MetricEventQueue;

	typedef EventBlock<MetricEventQueue> MetricBlock;
} // namespace MicromegasTracing
