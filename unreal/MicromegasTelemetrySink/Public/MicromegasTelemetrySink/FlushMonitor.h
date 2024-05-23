#pragma once
//
//  MicromegasTelemetrySink/FlushMonitor.h
//
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"

namespace MicromegasTracing
{
	class EventSink;
}

class MICROMEGASTELEMETRYSINK_API FlushMonitor
{
public:
	FlushMonitor();
	void Tick(MicromegasTracing::EventSink* sink);
	void Flush();

private:
	uint64 LastFlush;
	uint64 FlushDelay;
};

typedef TSharedPtr<FlushMonitor, ESPMode::ThreadSafe> SharedFlushMonitor;
