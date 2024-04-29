//
//  MicromegasTelemetrySink/FlushMonitor.cpp
//
#include "MicromegasTelemetrySink/FlushMonitor.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/ThreadStream.h"
#include "Misc/CoreDelegates.h"

namespace
{
	void MarkStreamFull(MicromegasTracing::ThreadStream* stream)
	{
		stream->MarkFull();
	}
} // namespace

FlushMonitor::FlushMonitor(MicromegasTracing::EventSink* sink)
{
	LastFlush = FPlatformTime::Cycles64();
	Sink = sink;
	double freq = 1.0 / FPlatformTime::GetSecondsPerCycle64();
	FlushDelay = static_cast<uint64>(freq * 60);
	FCoreDelegates::OnBeginFrame.AddRaw(this, &FlushMonitor::Tick);
}

FlushMonitor::~FlushMonitor()
{
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
}

void FlushMonitor::Tick()
{
	if (Sink->IsBusy())
	{
		return;
	}
	uint64 now = FPlatformTime::Cycles64();
	uint64 diff = now - LastFlush;
	if (diff > FlushDelay)
	{
		Flush();
		LastFlush = FPlatformTime::Cycles64();
	}
}

void FlushMonitor::Flush()
{
	MicromegasTracing::FlushLogStream();
	MicromegasTracing::FlushMetricStream();
	MicromegasTracing::ForEachThreadStream(&MarkStreamFull);
}
