#pragma once
//
//  MicromegasTracing/LogBlock.h
//
#include "MicromegasTracing/DualTime.h"
#include "MicromegasTracing/LogEvents.h"
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/EventBlock.h"

namespace MicromegasTracing
{
	typedef HeterogeneousQueue<
		LogStaticStrEvent,	   // cheapest log event, use when possible
		LogStringInteropEvent, // logs captured from UE_LOG
		StaticStringRef		   // not an event but necessary to parse events that reference a static string reference
		>
		LogEventQueue;

	typedef EventBlock<LogEventQueue> LogBlock;

} // namespace MicromegasTracing
