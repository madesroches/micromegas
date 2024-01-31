//
//  LgnTelemetrySink/FlushMonitor.cpp
//
#include "LgnTelemetrySink/FlushMonitor.h"
#include "LgnTracing/ThreadStream.h"
#include "LgnTracing/EventSink.h"

namespace
{
    void MarkStreamFull( LgnTracing::ThreadStream* stream )
    {
        stream->MarkFull();
    }
}

FlushMonitor::FlushMonitor(LgnTracing::EventSink* sink)
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
    if ( Sink->IsBusy() )
    {
        return;
    }
    uint64 now = FPlatformTime::Cycles64();
    uint64 diff = now - LastFlush;
    if ( diff > FlushDelay )
    {
        Flush();
        LastFlush = FPlatformTime::Cycles64();
    }
}

void FlushMonitor::Flush()
{
    using namespace LgnTracing;
    FlushLogStream();
    FlushMetricStream();
    ForEachThreadStream( &MarkStreamFull );
}
