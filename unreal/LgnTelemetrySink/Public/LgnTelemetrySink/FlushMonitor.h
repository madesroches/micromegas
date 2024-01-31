#pragma once
//
//  LgnTelemetrySink/FlushMonitor.h
//

namespace LgnTracing
{
    class EventSink;
}

class LGNTELEMETRYSINK_API FlushMonitor
{
public:
    explicit FlushMonitor(LgnTracing::EventSink* sink);
    ~FlushMonitor();

private:
    void Tick();
    void Flush();
    
    uint64 LastFlush;
    uint64 FlushDelay;
    LgnTracing::EventSink* Sink;
};
