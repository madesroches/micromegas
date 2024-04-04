#pragma once
//
//  LgnTracing/ThreadStream.h
//
#include "LgnTracing/ThreadBlock.h"
#include "LgnTracing/EventStream.h"

namespace LgnTracing
{
    typedef std::shared_ptr<ThreadBlock> ThreadsBlockPtr;
    typedef EventStreamImpl<ThreadBlock, 32> ThreadStream;
}

