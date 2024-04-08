#pragma once
//
//  MicromegasTracing/LogStream.h
//
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/EventStream.h"

namespace MicromegasTracing
{
	typedef std::shared_ptr<LogBlock> LogBlockPtr;
	typedef EventStreamImpl<LogBlock, 128> LogStream;

} // namespace MicromegasTracing
