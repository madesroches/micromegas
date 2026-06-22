#pragma once
//
//  MicromegasTracing/ImageBlock.h
//
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/HeterogeneousQueue.h"

namespace MicromegasTracing
{
	typedef EventBlock<ImageEventQueue> ImageBlock;
} // namespace MicromegasTracing
