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
	explicit FlushMonitor(MicromegasTracing::EventSink* sink);
	~FlushMonitor();

private:
	void Tick();
	void Flush();

	uint64 LastFlush;
	uint64 FlushDelay;
	MicromegasTracing::EventSink* Sink;
};
