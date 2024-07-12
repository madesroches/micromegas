#pragma once
//
//  MicromegasTelemetrySink/FlushMonitor.h
//
#include "HAL/IConsoleManager.h"
#include "HAL/Platform.h"
#include "Templates/SharedPointer.h"
#include "Templates/UniquePtr.h"

namespace MicromegasTracing
{
	class EventSink;
}

class MICROMEGASTELEMETRYSINK_API FlushMonitor
{
public:
	FlushMonitor();
	double Tick(MicromegasTracing::EventSink* sink); // returns the time until the next flush will be expected in seconds
	void Flush();
	double GetFlushPeriodSeconds() const;

private:
	TUniquePtr<TAutoConsoleVariable<float>> CVarFlushPeriodSeconds;
	double LastFlush;
};

typedef TSharedPtr<FlushMonitor, ESPMode::ThreadSafe> SharedFlushMonitor;
