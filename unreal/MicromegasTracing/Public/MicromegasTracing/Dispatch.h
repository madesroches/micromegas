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

		static void InitCurrentThreadStream();
		static void Shutdown();
		static void FlushLogStream();
		static void FlushMetricStream();
		static void FlushCurrentThreadStream();
		static void LogInterop(const LogStringInteropEvent& Event);
		static void LogStaticStr(const LogStaticStrEvent& Event);
		static void IntMetric(const MetricMetadata* desc, uint64 value, uint64 timestamp);
		static void FloatMetric(const MetricMetadata* desc, double value, uint64 timestamp);
		static void BeginScope(const BeginThreadSpanEvent& Event);
		static void EndScope(const EndThreadSpanEvent& Event);
		static void BeginNamedSpan(const BeginThreadNamedSpanEvent& Event);
		static void EndNamedSpan(const EndThreadNamedSpanEvent& Event);

		static void ForEachThreadStream(ThreadStreamCallback Callback);

		template <typename T>
		static void QueueLogEntry(const T& Event);

		template <typename T>
		static void QueueMetric(const T& Event);

		template <typename T>
		static void QueueThreadEvent(const T& Event);

		static ThreadStream* GetCurrentThreadStream();

		static PropertySetStore* GetPropertySetStore();

		static DefaultContext* GetDefaultContext();

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

		PropertySetStore* PropertySets;
		DefaultContext* Ctx;
	};

	extern CORE_API Dispatch* GDispatch;

} // namespace MicromegasTracing
