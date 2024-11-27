#pragma once
//
//  MicromegasTracing/LogBlock.h
//
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/HeterogeneousQueue.h"

namespace MicromegasTracing
{
	typedef EventBlock<LogEventQueue> LogBlock;

} // namespace MicromegasTracing
