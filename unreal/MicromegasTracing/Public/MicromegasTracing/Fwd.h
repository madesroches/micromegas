#pragma once
//
//  MicromegasTracing/Fwd.h
//
#include "Templates/SharedPointer.h"

namespace MicromegasTracing
{
	class EventSink;
	typedef TSharedPtr<EventSink, ESPMode::ThreadSafe> EventSinkPtr;
	struct TaggedLogInteropEvent;
	struct TaggedLogString;
	struct MetricMetadata;
	struct TaggedIntegerMetricEvent;
	struct TaggedFloatMetricEvent;
	struct BeginThreadSpanEvent;
	struct EndThreadSpanEvent;
	struct BeginThreadNamedSpanEvent;
	struct EndThreadNamedSpanEvent;
	struct DualTime;
	template <typename EventBlockT, size_t BUFFER_PADDING>
	class EventStreamImpl;
	template <typename QueueT>
	class EventBlock;
	template <typename... TS>
	class HeterogeneousQueue;
	struct StaticStringRef;
	typedef HeterogeneousQueue<TaggedLogInteropEvent, TaggedLogString, StaticStringRef> LogEventQueue;
	typedef EventBlock<LogEventQueue> LogBlock;
	typedef TSharedPtr<LogBlock, ESPMode::ThreadSafe> LogBlockPtr;
	typedef EventStreamImpl<LogBlock, 1024> LogStream;
	typedef TSharedPtr<LogStream, ESPMode::ThreadSafe> LogStreamPtr;
	typedef HeterogeneousQueue<TaggedIntegerMetricEvent, TaggedFloatMetricEvent> MetricEventQueue;
	typedef EventBlock<MetricEventQueue> MetricBlock;
	typedef TSharedPtr<MetricBlock, ESPMode::ThreadSafe> MetricsBlockPtr;
	typedef EventStreamImpl<MetricBlock, 128> MetricStream;
	typedef TSharedPtr<MetricStream, ESPMode::ThreadSafe> MetricStreamPtr;
	struct ProcessInfo;
	typedef TSharedPtr<ProcessInfo, ESPMode::ThreadSafe> ProcessInfoPtr;
	typedef HeterogeneousQueue<BeginThreadSpanEvent, EndThreadSpanEvent, BeginThreadNamedSpanEvent, EndThreadNamedSpanEvent> ThreadEventQueue;
	typedef EventBlock<ThreadEventQueue> ThreadBlock;
	typedef TSharedPtr<ThreadBlock, ESPMode::ThreadSafe> ThreadBlockPtr;
	typedef EventStreamImpl<ThreadBlock, 128> ThreadStream;
	class PropertySetStore;
	class PropertySet;
	struct Property;
	class DefaultContext;
} // namespace MicromegasTracing
