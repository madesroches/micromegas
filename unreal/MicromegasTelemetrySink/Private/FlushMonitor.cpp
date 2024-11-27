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
	: CVarFlushPeriodSeconds(new TAutoConsoleVariable<float>( // console variables are not available in double
		TEXT("telemetry.auto_flush_period"),
		60,
		TEXT("Telemetry flush period in seconds")))
{
	LastFlush = FPlatformTime::Seconds();
}

double FlushMonitor::GetFlushPeriodSeconds() const
{
	return CVarFlushPeriodSeconds->GetValueOnAnyThread();
}

double FlushMonitor::Tick(MicromegasTracing::EventSink* sink)
{
	double Now = FPlatformTime::Seconds();
	double Period = GetFlushPeriodSeconds();
	double Next = LastFlush + Period;
	if (Now >= Next)
	{
		Flush();
		return Period;
	}
	return Next - Now;
}

void FlushMonitor::Flush()
{
	MicromegasTracing::Dispatch::FlushLogStream();
	MicromegasTracing::Dispatch::FlushMetricStream();
	MicromegasTracing::Dispatch::ForEachThreadStream(&MarkStreamFull);
	MicromegasTracing::Dispatch::FlushCurrentThreadStream();
	LastFlush = FPlatformTime::Seconds();
}
