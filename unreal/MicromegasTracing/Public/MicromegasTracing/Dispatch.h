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
		static void Init(NewGuid AllocNewGuid,
			const ProcessInfoPtr& ProcessInfo,
			const TSharedPtr<EventSink, ESPMode::ThreadSafe>& Sink,
			size_t LogBufferSize,
			size_t MetricBufferSize,
			size_t ThreadBufferSize);
		~Dispatch();

		friend CORE_API void Shutdown();
		friend CORE_API void FlushLogStream();
		friend CORE_API void FlushMetricStream();
		friend CORE_API void FlushCurrentThreadStream();
		friend CORE_API void LogInterop(const LogStringInteropEvent& Event);
		friend CORE_API void LogStaticStr(const LogStaticStrEvent& Event);
		friend CORE_API void IntMetric(const IntegerMetricEvent& Event);
		friend CORE_API void FloatMetric(const FloatMetricEvent& Event);
		friend CORE_API void BeginScope(const BeginThreadSpanEvent& Event);
		friend CORE_API void EndScope(const EndThreadSpanEvent& Event);
		friend CORE_API void BeginNamedSpan(const BeginThreadNamedSpanEvent& Event);
		friend CORE_API void EndNamedSpan(const EndThreadSpanEvent& Event);

		friend CORE_API void ForEachThreadStream(ThreadStreamCallback Callback);

		template <typename T>
		friend void QueueLogEntry(const T& Event);

		template <typename T>
		friend void QueueMetric(const T& Event);

		template <typename T>
		friend void QueueThreadEvent(const T& Event);

		friend ThreadStream* GetCurrentThreadStream();

	private:
		Dispatch(NewGuid AllocNewGuid,
			const ProcessInfoPtr& ProcessInfo,
			const TSharedPtr<EventSink, ESPMode::ThreadSafe>& Sink,
			size_t LogBufferSize,
			size_t MetricBufferSize,
			size_t ThreadBufferSize);

		void FlushLogStreamImpl(UE::FMutex& Mutex);
		void FlushMetricStreamImpl(UE::FMutex& Mutex);
		void FlushThreadStream(ThreadStream* Stream);
		ThreadStream* AllocThreadStream();
		void PublishThreadStream(ThreadStream* Stream);

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
	CORE_API void LogInterop(const LogStringInteropEvent& Event);
	CORE_API void ForEachThreadStream(ThreadStreamCallback Callback);
	CORE_API void BeginNamedSpan(const BeginThreadNamedSpanEvent& Event);
	CORE_API void EndNamedSpan(const EndThreadNamedSpanEvent& Event);

} // namespace MicromegasTracing
