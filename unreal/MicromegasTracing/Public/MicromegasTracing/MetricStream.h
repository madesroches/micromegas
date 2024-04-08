#pragma once
//
//  MicromegasTracing/MetricStream.h
//
#include "MicromegasTracing/MetricBlock.h"
#include "MicromegasTracing/EventStream.h"

namespace MicromegasTracing
{
	typedef std::shared_ptr<MetricBlock> MetricsBlockPtr;
	typedef EventStreamImpl<MetricBlock, 32> MetricStream;
} // namespace MicromegasTracing
