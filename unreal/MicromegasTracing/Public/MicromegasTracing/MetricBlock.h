#pragma once
//
//  LgnTracing/MetricBlock.h
//
#include "LgnTracing/EventBlock.h"
#include "LgnTracing/MetricEvents.h"

namespace LgnTracing
{
    typedef HeterogeneousQueue<
        IntegerMetricEvent,
        FloatMetricEvent
        > MetricEventQueue;

    typedef EventBlock<MetricEventQueue> MetricBlock;
}

