#pragma once
//
//  LgnTracing/LogStream.h
//
#include "LgnTracing/LogBlock.h"
#include "LgnTracing/EventStream.h"

namespace LgnTracing
{
    typedef std::shared_ptr<LogBlock> LogBlockPtr;
    typedef EventStreamImpl<LogBlock, 128> LogStream;

} // namespace
