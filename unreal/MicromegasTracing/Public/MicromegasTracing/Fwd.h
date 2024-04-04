#pragma once
//
//  MicromegasTracing/Fwd.h
//
#include <memory>

namespace MicromegasTracing
{
    class EventSink;
    struct LogStringInteropEvent;
    struct LogStaticStrEvent;
    struct IntegerMetricEvent;
    struct FloatMetricEvent;
    struct BeginThreadSpanEvent;
    struct EndThreadSpanEvent;
    struct DualTime;
    template< typename EventBlockT, size_t BUFFER_PADDING >
    class EventStreamImpl;
    template< typename QueueT >
    class EventBlock;
    template< typename... TS >
    class HeterogeneousQueue;
    struct StaticStringRef;
    typedef HeterogeneousQueue<LogStaticStrEvent, LogStringInteropEvent, StaticStringRef> LogEventQueue;
    typedef EventBlock<LogEventQueue> LogBlock;
    typedef std::shared_ptr<LogBlock> LogBlockPtr;
    typedef EventStreamImpl<LogBlock, 128> LogStream;
    typedef std::shared_ptr<LogStream> LogStreamPtr;
    typedef HeterogeneousQueue<IntegerMetricEvent, FloatMetricEvent> MetricEventQueue;
    typedef EventBlock<MetricEventQueue> MetricBlock;
    typedef std::shared_ptr<MetricBlock> MetricsBlockPtr;
    typedef EventStreamImpl<MetricBlock, 32> MetricStream;
    typedef std::shared_ptr<MetricStream> MetricStreamPtr;
    struct ProcessInfo;
    typedef std::shared_ptr<ProcessInfo> ProcessInfoPtr;
    typedef HeterogeneousQueue<BeginThreadSpanEvent,EndThreadSpanEvent> ThreadEventQueue;
    typedef EventBlock<ThreadEventQueue> ThreadBlock;
    typedef std::shared_ptr<ThreadBlock> ThreadBlockPtr;
    typedef EventStreamImpl<ThreadBlock, 32> ThreadStream;
}
