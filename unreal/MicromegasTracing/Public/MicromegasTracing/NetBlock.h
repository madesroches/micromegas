#pragma once
//
//  MicromegasTracing/NetBlock.h
//
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/HeterogeneousQueue.h"

namespace MicromegasTracing
{
	typedef EventBlock<NetEventQueue> NetBlock;
} // namespace MicromegasTracing
