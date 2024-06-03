//
//  MicromegasTelemetrySink/FlushMonitor.cpp
//
#include "MicromegasTelemetrySink/FlushMonitor.h"
#include "HAL/PlatformTime.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/EventStream.h"

namespace
{
	void MarkStreamFull(MicromegasTracing::ThreadStream* stream)
	{
		stream->MarkFull();
	}
} // namespace

FlushMonitor::FlushMonitor()
{
	LastFlush = FPlatformTime::Cycles64();
	double freq = 1.0 / FPlatformTime::GetSecondsPerCycle64();
	FlushDelay = static_cast<uint64>(freq * 60);
}

void FlushMonitor::Tick(MicromegasTracing::EventSink* sink)
{
	if (sink->IsBusy())
	{
		return;
	}
	uint64 now = FPlatformTime::Cycles64();
	uint64 diff = now - LastFlush;
	if (diff > FlushDelay)
	{
		Flush();
	}
}

void FlushMonitor::Flush()
{
	MicromegasTracing::FlushLogStream();
	MicromegasTracing::FlushMetricStream();
	MicromegasTracing::ForEachThreadStream(&MarkStreamFull);
	MicromegasTracing::FlushCurrentThreadStream();
	LastFlush = FPlatformTime::Cycles64();
}
