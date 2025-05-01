#pragma once
//
//  MicromegasTracing/Dispatch.h
//
#include "Async/Mutex.h"
#include "Containers/Array.h"
#include "Containers/UnrealString.h"
#include "HAL/Platform.h"
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/LogEvents.h"
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
		static void LogInterop(uint64 Timestamp, LogLevel::Type InLevel, const StaticStringRef& InTarget, const DynamicString& Msg);
		static void Log(const LogMetadata* Desc, uint64 Timestamp, const DynamicString& Msg);
		static void Log(const LogMetadata* Desc, const PropertySet* Properties, uint64 Timestamp, const DynamicString& Msg);
		static void IntMetric(const MetricMetadata* Desc, uint64 Value, uint64 Timestamp);
		static void IntMetric(const MetricMetadata* Desc, const PropertySet* Properties, uint64 Value, uint64 Timestamp);
		static void FloatMetric(const MetricMetadata* Desc, double Value, uint64 Timestamp);
		static void FloatMetric(const MetricMetadata* Desc, const PropertySet* Properties, double Value, uint64 Timestamp);
		static void BeginScope(const BeginThreadSpanEvent& Event);
		static void EndScope(const EndThreadSpanEvent& Event);
		static void BeginNamedSpan(const BeginThreadNamedSpanEvent& Event);
		static void EndNamedSpan(const EndThreadNamedSpanEvent& Event);

		static void ForEachThreadStream(ThreadStreamCallback Callback);

		template <typename T>
		void QueueLogEntry(const T& Event);

		template <typename T>
		void QueueMetric(const T& Event);

		template <typename T>
		static void QueueThreadEvent(const T& Event);

		static ThreadStream* GetCurrentThreadStream();

		static PropertySetStore* GetPropertySetStore();

		static DefaultContext* GetDefaultContext();

		static const PropertySet* GetPropertySet(const TMap<FName, FName>& Context);

		static ProcessInfoConstPtr GetCurrentProcessInfo();

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
