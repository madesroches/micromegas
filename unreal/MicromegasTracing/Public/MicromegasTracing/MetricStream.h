#pragma once
//
//  LgnTracing/MetricStream.h
//
#include "LgnTracing/MetricBlock.h"
#include "LgnTracing/EventStream.h"

namespace LgnTracing
{
    typedef std::shared_ptr<MetricBlock> MetricsBlockPtr;
    typedef EventStreamImpl<MetricBlock, 32> MetricStream;
}

