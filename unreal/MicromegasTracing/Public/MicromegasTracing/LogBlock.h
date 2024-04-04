#pragma once
//
//  LgnTracing/LogBlock.h
//
#include "LgnTracing/DualTime.h"
#include "LgnTracing/LogEvents.h"
#include "LgnTracing/HeterogeneousQueue.h"
#include "LgnTracing/EventBlock.h"

namespace LgnTracing
{
    typedef HeterogeneousQueue<
        LogStaticStrEvent,     // cheapest log event, use when possible
        LogStringInteropEvent, // logs captured from UE_LOG
        StaticStringRef        // not an event but necessary to parse events that reference a static string reference
        > LogEventQueue;

    typedef EventBlock<LogEventQueue> LogBlock;

} // namespace
