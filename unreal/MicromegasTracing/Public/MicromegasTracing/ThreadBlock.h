#pragma once
//
//  LgnTracing/ThreadBlock.h
//
#include "LgnTracing/EventBlock.h"
#include "LgnTracing/SpanEvents.h"

namespace LgnTracing
{
    typedef HeterogeneousQueue<
        BeginThreadSpanEvent,
        EndThreadSpanEvent
        > ThreadEventQueue;

    typedef EventBlock<ThreadEventQueue> ThreadBlock;
}

