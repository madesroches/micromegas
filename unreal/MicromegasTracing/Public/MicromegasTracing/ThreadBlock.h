#pragma once
//
//  MicromegasTracing/ThreadBlock.h
//
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/SpanEvents.h"

namespace MicromegasTracing
{
    typedef HeterogeneousQueue<
        BeginThreadSpanEvent,
        EndThreadSpanEvent
        > ThreadEventQueue;

    typedef EventBlock<ThreadEventQueue> ThreadBlock;
}

