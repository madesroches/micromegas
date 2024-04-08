#pragma once
//
//  MicromegasTracing/ThreadStream.h
//
#include "MicromegasTracing/ThreadBlock.h"
#include "MicromegasTracing/EventStream.h"

namespace MicromegasTracing
{
	typedef std::shared_ptr<ThreadBlock> ThreadsBlockPtr;
	typedef EventStreamImpl<ThreadBlock, 32> ThreadStream;
} // namespace MicromegasTracing
