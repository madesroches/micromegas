#pragma once
//
//  MicromegasTelemetrySink/FlushMonitor.h
//
#include "HAL/Platform.h"

namespace MicromegasTracing
{
	class EventSink;
}

class MICROMEGASTELEMETRYSINK_API FlushMonitor
{
public:
	FlushMonitor();
	void Tick(MicromegasTracing::EventSink* sink);

private:
	void Flush();

	uint64 LastFlush;
	uint64 FlushDelay;
};
