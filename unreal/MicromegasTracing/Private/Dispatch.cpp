//
//  MicromegasTracing/Dispatch.cpp
//
#include "MicromegasTracing/Dispatch.h"
#include "Async/UniqueLock.h"
#include "HAL/PlatformProcess.h"
#include "MicromegasTracing/DefaultContext.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/EventStream.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/MetricEvents.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "MicromegasTracing/PropertySetStore.h"
#include "MicromegasTracing/SpanEvents.h"
#include "Misc/Guid.h"
#include "Misc/ScopeLock.h"

namespace MicromegasTracing
{
	Dispatch* GDispatch = nullptr;

	Dispatch::Dispatch(NewGuid InAllocNewGuid,
		const ProcessInfoPtr& ProcessInfo,
		const TSharedPtr<EventSink, ESPMode::ThreadSafe>& InSink,
		size_t InLogBufferSize,
		size_t InMetricBufferSize,
		size_t InThreadBufferSize)
		: AllocNewGuid(InAllocNewGuid)
		, Sink(InSink)
		, CurrentProcessInfo(ProcessInfo)
		, LogBufferSize(InLogBufferSize)
		, MetricBufferSize(InMetricBufferSize)
		, ThreadBufferSize(InThreadBufferSize)
		, PropertySets(new PropertySetStore())
		, Ctx(new DefaultContext(PropertySets))
	{
		FString LogStreamId = AllocNewGuid();
		LogBlockPtr NewLogBlock = MakeShared<LogBlock>(LogStreamId,
			ProcessInfo->StartTime,
			LogBufferSize,
			0);
		LogEntries = MakeShared<LogStream>(CurrentProcessInfo->ProcessId,
			LogStreamId,
			NewLogBlock,
			TArray<FString>({ TEXT("log") }));

		FString MetricStreamId = AllocNewGuid();
		MetricsBlockPtr NewMetricBlock = MakeShared<MetricBlock>(MetricStreamId,
			ProcessInfo->StartTime,
			MetricBufferSize,
			0);
		Metrics = MakeShared<MetricStream>(CurrentProcessInfo->ProcessId,
			MetricStreamId,
			NewMetricBlock,
			TArray<FString>({ TEXT("metrics") }));
	}

	Dispatch::~Dispatch()
	{
	}

	void Dispatch::Init(NewGuid AllocNewGuid,
		const ProcessInfoPtr& ProcessInfo,
		const TSharedPtr<EventSink, ESPMode::ThreadSafe>& Sink,
		size_t LogBufferSize,
		size_t MetricBufferSize,
		size_t ThreadBufferSize)
	{
		if (GDispatch)
		{
			return;
		}
		GDispatch = new Dispatch(AllocNewGuid, ProcessInfo, Sink, LogBufferSize, MetricBufferSize, ThreadBufferSize);
		Sink->OnStartup(ProcessInfo);
		Sink->OnInitLogStream(GDispatch->LogEntries);
		Sink->OnInitMetricStream(GDispatch->Metrics);
	}

	void Dispatch::FlushLogStreamImpl(UE::FMutex& Mutex)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (LogEntries->GetCurrentBlock().IsEmpty())
		{
			Mutex.Unlock();
			return;
		}
		DualTime Now = DualTime::Now();
		size_t NewOffset = LogEntries->GetCurrentBlock().GetOffset() + LogEntries->GetCurrentBlock().GetEvents().GetNbEvents();
		LogBlockPtr NewBlock = MakeShared<LogBlock>(LogEntries->GetStreamId(),
			Now,
			LogBufferSize,
			NewOffset);
		LogBlockPtr FullBlock = LogEntries->SwapBlocks(NewBlock);
		FullBlock->Close(Now);
		Mutex.Unlock();
		Sink->OnProcessLogBlock(FullBlock);
	}

	void Dispatch::FlushMetricStreamImpl(UE::FMutex& Mutex)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (Metrics->GetCurrentBlock().IsEmpty())
		{
			Mutex.Unlock();
			return;
		}
		DualTime Now = DualTime::Now();
		size_t NewOffset = Metrics->GetCurrentBlock().GetOffset() + Metrics->GetCurrentBlock().GetEvents().GetNbEvents();
		MetricsBlockPtr NewBlock = MakeShared<MetricBlock>(Metrics->GetStreamId(),
			Now,
			MetricBufferSize,
			NewOffset);
		MetricsBlockPtr FullBlock = Metrics->SwapBlocks(NewBlock);
		FullBlock->Close(Now);
		Mutex.Unlock();
		Sink->OnProcessMetricBlock(FullBlock);
	}

	void Dispatch::FlushThreadStream(ThreadStream* Stream)
	{
		if (Stream->GetCurrentBlock().IsEmpty())
		{
			return;
		}
		DualTime Now = DualTime::Now();
		size_t NewOffset = Stream->GetCurrentBlock().GetOffset() + Stream->GetCurrentBlock().GetEvents().GetNbEvents();
		ThreadBlockPtr NewBlock = MakeShared<ThreadBlock>(Stream->GetStreamId(),
			Now,
			ThreadBufferSize,
			NewOffset);
		ThreadBlockPtr FullBlock = Stream->SwapBlocks(NewBlock);
		FullBlock->Close(Now);
		Sink->OnProcessThreadBlock(FullBlock);
	}

	ThreadStream* Dispatch::AllocThreadStream()
	{
		FString StreamId = AllocNewGuid();
		DualTime Now = DualTime::Now();
		ThreadBlockPtr Block = MakeShared<ThreadBlock>(StreamId,
			Now,
			ThreadBufferSize,
			0);
		return new ThreadStream(CurrentProcessInfo->ProcessId,
			StreamId,
			Block,
			TArray<FString>({ TEXT("cpu") }));
	}

	void Dispatch::PublishThreadStream(ThreadStream* Stream)
	{
		{
			UE::TUniqueLock<UE::FMutex> Llock(ThreadStreamsMutex);
			ThreadStreams.Add(Stream);
		}
		Sink->OnInitThreadStream(Stream);
	}

	template <typename T>
	void Dispatch::QueueLogEntry(const T& Event)
	{
		LogMutex.Lock();
		LogEntries->GetCurrentBlock().GetEvents().Push(Event);
		if (LogEntries->IsFull())
		{
			FlushLogStreamImpl(LogMutex); // unlocks the mutex
		}
		else
		{
			LogMutex.Unlock();
		}
	}

	void Dispatch::FlushLogStream()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->LogMutex.Lock();
		Dispatch->FlushLogStreamImpl(Dispatch->LogMutex); // unlocks the mutex
	}

	void Dispatch::FlushMetricStream()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->MetricMutex.Lock();
		Dispatch->FlushMetricStreamImpl(Dispatch->MetricMutex); // unlocks the mutex
	}

	void Dispatch::Shutdown()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->Sink->OnShutdown();
		GDispatch = nullptr;
	}

	void Dispatch::LogInterop(uint64 Timestamp, LogLevel::Type Level, const StaticStringRef& Target, const DynamicString& Msg)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueLogEntry(TaggedLogInteropEvent(Timestamp, Level, Target, Dispatch->Ctx->GetCurrentPropertySet(), Msg));
	}

	void Dispatch::Log(const LogMetadata* Desc, uint64 Timestamp, const DynamicString& Msg)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueLogEntry(TaggedLogString(Desc, Dispatch->Ctx->GetCurrentPropertySet(), Timestamp, Msg));
	}

	void Dispatch::Log(const LogMetadata* Desc, const PropertySet* Properties, uint64 Timestamp, const DynamicString& Msg)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueLogEntry(TaggedLogString(Desc, Properties, Timestamp, Msg));
	}

	template <typename T>
	void Dispatch::QueueMetric(const T& Event)
	{
		MetricMutex.Lock();
		Metrics->GetCurrentBlock().GetEvents().Push(Event);
		if (Metrics->IsFull())
		{
			FlushMetricStreamImpl(MetricMutex); // unlocks the mutex
		}
		else
		{
			MetricMutex.Unlock();
		}
	}

	void Dispatch::IntMetric(const MetricMetadata* Desc, uint64 Value, uint64 Timestamp)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueMetric(TaggedIntegerMetricEvent(Desc, Dispatch->Ctx->GetCurrentPropertySet(), Value, Timestamp));
	}

	void Dispatch::IntMetric(const MetricMetadata* Desc, const PropertySet* Properties, uint64 Value, uint64 Timestamp)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueMetric(TaggedIntegerMetricEvent(Desc, Properties, Value, Timestamp));
	}

	void Dispatch::FloatMetric(const MetricMetadata* Desc, double Value, uint64 Timestamp)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueMetric(TaggedFloatMetricEvent(Desc, Dispatch->Ctx->GetCurrentPropertySet(), Value, Timestamp));
	}

	void Dispatch::FloatMetric(const MetricMetadata* Desc, const PropertySet* Properties, double Value, uint64 Timestamp)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		Dispatch->QueueMetric(TaggedFloatMetricEvent(Desc, Properties, Value, Timestamp));
	}

	ThreadStream* Dispatch::GetCurrentThreadStream()
	{
		thread_local ThreadStream* Ptr = nullptr;
		if (Ptr)
		{
			return Ptr;
		}
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return nullptr;
		}
		thread_local bool ThisStreamBeingInit = false;
		if (ThisStreamBeingInit)
		{
			return nullptr;
		}
		ThisStreamBeingInit = true;
		ThreadStream* NewStream = Dispatch->AllocThreadStream();
		Dispatch->PublishThreadStream(NewStream);
		Ptr = NewStream; // starting from now events can be queued
		return Ptr;
	}

	template <typename T>
	void Dispatch::QueueThreadEvent(const T& Event)
	{
		if (ThreadStream* Stream = GetCurrentThreadStream())
		{
			Stream->GetCurrentBlock().GetEvents().Push(Event);
			if (Stream->IsFull())
			{
				Dispatch* Dispatch = GDispatch;
				if (!Dispatch)
				{
					return;
				}
				Dispatch->FlushThreadStream(Stream);
			}
		}
	}

	void Dispatch::BeginScope(const BeginThreadSpanEvent& Event)
	{
		QueueThreadEvent(Event);
	}

	void Dispatch::EndScope(const EndThreadSpanEvent& Event)
	{
		QueueThreadEvent(Event);
	}

	void Dispatch::BeginNamedSpan(const BeginThreadNamedSpanEvent& Event)
	{
		QueueThreadEvent(Event);
	}

	void Dispatch::EndNamedSpan(const EndThreadNamedSpanEvent& Event)
	{
		QueueThreadEvent(Event);
	}

	void Dispatch::ForEachThreadStream(ThreadStreamCallback Callback)
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		UE::TUniqueLock<UE::FMutex> Lock(Dispatch->ThreadStreamsMutex);
		for (ThreadStream* Stream : Dispatch->ThreadStreams)
		{
			Callback(Stream);
		}
	}

	void Dispatch::InitCurrentThreadStream()
	{
		// Thread streams will be implicitly initialized as soon as they emit events,
		// but the first event's timestamp will be before the beginning of the block
		// (since it will be allocated after that event).
		// This could confuse some tooling. Calling InitCurrentThreadStream() explicitly before
		// events are emitted prevents this problem.
		Dispatch::GetCurrentThreadStream();
	}

	void Dispatch::FlushCurrentThreadStream()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return;
		}
		ThreadStream* Stream = GetCurrentThreadStream();
		if (!Stream)
		{
			return;
		}
		Dispatch->FlushThreadStream(Stream);
	}

	PropertySetStore* Dispatch::GetPropertySetStore()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return nullptr;
		}
		return Dispatch->PropertySets;
	}

	DefaultContext* Dispatch::GetDefaultContext()
	{
		Dispatch* Dispatch = GDispatch;
		if (!Dispatch)
		{
			return nullptr;
		}
		return Dispatch->Ctx;
	}

	const PropertySet* Dispatch::GetPropertySet(const TMap<FName, FName>& Context)
	{
		if (PropertySetStore* store = GetPropertySetStore())
		{
			return store->Get(Context);
		}
		return nullptr;
	}

} // namespace MicromegasTracing
