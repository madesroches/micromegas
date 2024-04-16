#pragma once
//
//  MicromegasTracing/Dispatch.h
//
#include <string>
#include <memory>
#include <mutex>
#include <vector>
#include "HAL/Platform.h"
#include "MicromegasTracing/Fwd.h"
class FScopeLock;

namespace MicromegasTracing
{
	typedef FString (*NewGuid)();
	typedef void (*ThreadStreamCallback)(ThreadStream*);

	class CORE_API Dispatch
	{
	public:
		static void Init(NewGuid allocNewGuid,
			const ProcessInfoPtr& processInfo,
			const std::shared_ptr<EventSink>& sink,
			size_t logBufferSize,
			size_t metricBufferSize,
			size_t threadBufferSize);
		~Dispatch();

		friend CORE_API void Shutdown();
		friend CORE_API void FlushLogStream();
		friend CORE_API void FlushMetricStream();
		friend CORE_API void LogInterop(const LogStringInteropEvent& event);
		friend CORE_API void LogStaticStr(const LogStaticStrEvent& event);
		friend CORE_API void IntMetric(const IntegerMetricEvent& event);
		friend CORE_API void FloatMetric(const FloatMetricEvent& event);
		friend CORE_API void BeginScope(const BeginThreadSpanEvent& event);
		friend CORE_API void EndScope(const EndThreadSpanEvent& event);

		friend CORE_API void ForEachThreadStream(ThreadStreamCallback callback);

		template <typename T>
		friend void QueueLogEntry(const T& event);

		template <typename T>
		friend void QueueMetric(const T& event);

		template <typename T>
		friend void QueueThreadEvent(const T& event);

		friend ThreadStream* GetCurrentThreadStream();

	private:
		Dispatch(NewGuid allocNewGuid,
			const std::shared_ptr<EventSink>& sink,
			const ProcessInfoPtr& processInfo,
			size_t logBufferSize,
			size_t metricBufferSize,
			size_t threadBufferSize);

		typedef std::unique_ptr<std::lock_guard<std::recursive_mutex>> GuardPtr;
		void FlushLogStreamImpl(GuardPtr& guard);
		void FlushMetricStreamImpl(GuardPtr& guard);
		void FlushThreadStream(ThreadStream* stream);
		ThreadStream* AllocThreadStream();
		void PublishThreadStream(ThreadStream* stream);

		NewGuid AllocNewGuid;

		std::shared_ptr<EventSink> Sink;
		ProcessInfoPtr CurrentProcessInfo;

		std::recursive_mutex LogMutex;
		std::shared_ptr<LogStream> LogEntries;
		size_t LogBufferSize;

		std::recursive_mutex MetricMutex;
		std::shared_ptr<MetricStream> Metrics;
		size_t MetricBufferSize;

		std::recursive_mutex ThreadStreamsMutex;
		std::vector<ThreadStream*> ThreadStreams;
		size_t ThreadBufferSize;
	};

	extern CORE_API Dispatch* GDispatch;

	CORE_API void FlushLogStream();
	CORE_API void FlushMetricStream();
	CORE_API void LogInterop(const LogStringInteropEvent& event);
	CORE_API void ForEachThreadStream(ThreadStreamCallback callback);

} // namespace MicromegasTracing
