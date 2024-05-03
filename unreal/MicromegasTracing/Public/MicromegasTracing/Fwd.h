#pragma once
//
//  MicromegasTracing/Fwd.h
//
#include "Templates/SharedPointer.h"

namespace MicromegasTracing
{
	class EventSink;
	typedef TSharedPtr<EventSink, ESPMode::ThreadSafe> EventSinkPtr;
	struct LogStringInteropEvent;
	struct LogStaticStrEvent;
	struct IntegerMetricEvent;
	struct FloatMetricEvent;
	struct BeginThreadSpanEvent;
	struct EndThreadSpanEvent;
	struct DualTime;
	template <typename EventBlockT, size_t BUFFER_PADDING>
	class EventStreamImpl;
	template <typename QueueT>
	class EventBlock;
	template <typename... TS>
	class HeterogeneousQueue;
	struct StaticStringRef;
	typedef HeterogeneousQueue<LogStaticStrEvent, LogStringInteropEvent, StaticStringRef> LogEventQueue;
	typedef EventBlock<LogEventQueue> LogBlock;
	typedef TSharedPtr<LogBlock, ESPMode::ThreadSafe> LogBlockPtr;
	typedef EventStreamImpl<LogBlock, 128> LogStream;
	typedef TSharedPtr<LogStream, ESPMode::ThreadSafe> LogStreamPtr;
	typedef HeterogeneousQueue<IntegerMetricEvent, FloatMetricEvent> MetricEventQueue;
	typedef EventBlock<MetricEventQueue> MetricBlock;
	typedef TSharedPtr<MetricBlock, ESPMode::ThreadSafe> MetricsBlockPtr;
	typedef EventStreamImpl<MetricBlock, 32> MetricStream;
	typedef TSharedPtr<MetricStream, ESPMode::ThreadSafe> MetricStreamPtr;
	struct ProcessInfo;
	typedef TSharedPtr<ProcessInfo, ESPMode::ThreadSafe> ProcessInfoPtr;
	typedef HeterogeneousQueue<BeginThreadSpanEvent, EndThreadSpanEvent> ThreadEventQueue;
	typedef EventBlock<ThreadEventQueue> ThreadBlock;
	typedef TSharedPtr<ThreadBlock, ESPMode::ThreadSafe> ThreadBlockPtr;
	typedef EventStreamImpl<ThreadBlock, 32> ThreadStream;
} // namespace MicromegasTracing
