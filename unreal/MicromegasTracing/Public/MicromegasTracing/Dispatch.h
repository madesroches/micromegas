#pragma once
//
//  MicromegasTracing/Dispatch.h
//
#include "Async/Mutex.h"
#include "Containers/UnrealString.h"
#include "HAL/Platform.h"
#include "MicromegasTracing/Fwd.h"
#include "Templates/SharedPointer.h"
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
			const TSharedPtr<EventSink, ESPMode::ThreadSafe>& sink,
			size_t logBufferSize,
			size_t metricBufferSize,
			size_t threadBufferSize);
		~Dispatch();

		friend CORE_API void Shutdown();
		friend CORE_API void FlushLogStream();
		friend CORE_API void FlushMetricStream();
		friend CORE_API void FlushCurrentThreadStream();
		friend CORE_API void LogInterop(const LogStringInteropEvent& event);
		friend CORE_API void LogStaticStr(const LogStaticStrEvent& event);
		friend CORE_API void IntMetric(const IntegerMetricEvent& event);
		friend CORE_API void FloatMetric(const FloatMetricEvent& event);
		friend CORE_API void BeginScope(const BeginThreadSpanEvent& event);
		friend CORE_API void EndScope(const EndThreadSpanEvent& event);
		friend CORE_API void BeginNamedSpan(const BeginThreadNamedSpanEvent& event);
		friend CORE_API void EndNamedSpan(const EndThreadSpanEvent& event);

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
			const ProcessInfoPtr& processInfo,
			const TSharedPtr<EventSink, ESPMode::ThreadSafe>& sink,
			size_t logBufferSize,
			size_t metricBufferSize,
			size_t threadBufferSize);

		void FlushLogStreamImpl(UE::FMutex& mutex);
		void FlushMetricStreamImpl(UE::FMutex& mutex);
		void FlushThreadStream(ThreadStream* stream);
		ThreadStream* AllocThreadStream();
		void PublishThreadStream(ThreadStream* stream);

		NewGuid AllocNewGuid;

		TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink;
		ProcessInfoPtr CurrentProcessInfo;

		UE::FMutex LogMutex;
		LogStreamPtr LogEntries;
		size_t LogBufferSize;

		UE::FMutex MetricMutex;
		MetricStreamPtr Metrics;
		size_t MetricBufferSize;

		UE::FMutex ThreadStreamsMutex;
		TArray<ThreadStream*> ThreadStreams;
		size_t ThreadBufferSize;
	};

	extern CORE_API Dispatch* GDispatch;

	CORE_API void Shutdown();
	CORE_API void FlushLogStream();
	CORE_API void FlushMetricStream();
	CORE_API void InitCurrentThreadStream();
	CORE_API void FlushCurrentThreadStream();
	CORE_API void LogInterop(const LogStringInteropEvent& event);
	CORE_API void ForEachThreadStream(ThreadStreamCallback callback);
	CORE_API void BeginNamedSpan(const BeginThreadNamedSpanEvent& event);
	CORE_API void EndNamedSpan(const EndThreadNamedSpanEvent& event);

} // namespace MicromegasTracing
