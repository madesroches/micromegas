#pragma once
//
//  MicromegasTracing/MetricBlock.h
//
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/MetricEvents.h"

namespace MicromegasTracing
{
	typedef EventBlock<MetricEventQueue> MetricBlock;
} // namespace MicromegasTracing
