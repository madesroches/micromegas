//
//  MicromegasTracing/Dispatch.cpp
//
#include "MicromegasTracing/Dispatch.h"
#include "Async/UniqueLock.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/Guid.h"
#include "Misc/ScopeLock.h"
#include "HAL/PlatformProcess.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/LogStream.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/MetricEvents.h"
#include "MicromegasTracing/SpanEvents.h"

namespace MicromegasTracing
{
	Dispatch* GDispatch = nullptr;

	Dispatch::Dispatch(NewGuid allocNewGuid,
		const ProcessInfoPtr& processInfo,
		const TSharedPtr<EventSink, ESPMode::ThreadSafe>& sink,
		size_t logBufferSize,
		size_t metricBufferSize,
		size_t threadBufferSize)
		: AllocNewGuid(allocNewGuid)
		, Sink(sink)
		, CurrentProcessInfo(processInfo)
		, LogBufferSize(logBufferSize)
		, MetricBufferSize(metricBufferSize)
		, ThreadBufferSize(threadBufferSize)
	{
		FString logStreamId = AllocNewGuid();
		LogBlockPtr logBlock = MakeShared<LogBlock>(logStreamId,
			processInfo->StartTime,
			LogBufferSize,
			0);
		LogEntries = MakeShared<LogStream>(CurrentProcessInfo->ProcessId,
			logStreamId,
			logBlock,
			TArray<FString>({ TEXT("log") }));

		FString metricStreamId = allocNewGuid();
		MetricsBlockPtr metricBlock = MakeShared<MetricBlock>(metricStreamId,
			processInfo->StartTime,
			metricBufferSize,
			0);
		Metrics = MakeShared<MetricStream>(CurrentProcessInfo->ProcessId,
			metricStreamId,
			metricBlock,
			TArray<FString>({ TEXT("metrics") }));
	}

	Dispatch::~Dispatch()
	{
	}

	void Dispatch::Init(NewGuid allocNewGuid,
		const ProcessInfoPtr& processInfo,
		const TSharedPtr<EventSink, ESPMode::ThreadSafe>& sink,
		size_t logBufferSize,
		size_t metricBufferSize,
		size_t threadBufferSize)
	{
		if (GDispatch)
		{
			return;
		}
		GDispatch = new Dispatch(allocNewGuid, processInfo, sink, logBufferSize, metricBufferSize, threadBufferSize);
		sink->OnStartup(processInfo);
		sink->OnInitLogStream(GDispatch->LogEntries);
		sink->OnInitMetricStream(GDispatch->Metrics);
	}

	void Dispatch::FlushLogStreamImpl(UE::FMutex& mutex)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (LogEntries->GetCurrentBlock().IsEmpty())
		{
			mutex.Unlock();
			return;
		}
		DualTime now = DualTime::Now();
		size_t new_offset = LogEntries->GetCurrentBlock().GetOffset() + LogEntries->GetCurrentBlock().GetEvents().GetNbEvents();
		LogBlockPtr newBlock = MakeShared<LogBlock>(LogEntries->GetStreamId(),
			now,
			LogBufferSize,
			new_offset);
		LogBlockPtr fullBlock = LogEntries->SwapBlocks(newBlock);
		fullBlock->Close(now);
		mutex.Unlock();
		Sink->OnProcessLogBlock(fullBlock);
	}

	void Dispatch::FlushMetricStreamImpl(UE::FMutex& mutex)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (Metrics->GetCurrentBlock().IsEmpty())
		{
			mutex.Unlock();
			return;
		}
		DualTime now = DualTime::Now();
		size_t new_offset = Metrics->GetCurrentBlock().GetOffset() + Metrics->GetCurrentBlock().GetEvents().GetNbEvents();
		MetricsBlockPtr newBlock = MakeShared<MetricBlock>(Metrics->GetStreamId(),
			now,
			MetricBufferSize,
			new_offset);
		MetricsBlockPtr fullBlock = Metrics->SwapBlocks(newBlock);
		fullBlock->Close(now);
		mutex.Unlock();
		Sink->OnProcessMetricBlock(fullBlock);
	}

	void Dispatch::FlushThreadStream(ThreadStream* stream)
	{
		if (stream->GetCurrentBlock().IsEmpty())
		{
			return;
		}
		DualTime now = DualTime::Now();
		size_t new_offset = stream->GetCurrentBlock().GetOffset() + stream->GetCurrentBlock().GetEvents().GetNbEvents();
		ThreadBlockPtr newBlock = MakeShared<ThreadBlock>(stream->GetStreamId(),
			now,
			ThreadBufferSize,
			new_offset);
		ThreadBlockPtr fullBlock = stream->SwapBlocks(newBlock);
		fullBlock->Close(now);
		Sink->OnProcessThreadBlock(fullBlock);
	}

	ThreadStream* Dispatch::AllocThreadStream()
	{
		FString streamId = AllocNewGuid();
		DualTime now = DualTime::Now();
		ThreadBlockPtr block = MakeShared<ThreadBlock>(streamId,
			now,
			ThreadBufferSize,
			0);
		return new ThreadStream(CurrentProcessInfo->ProcessId,
			streamId,
			block,
			TArray<FString>({ TEXT("cpu") }));
	}

	void Dispatch::PublishThreadStream(ThreadStream* stream)
	{
		{
			UE::TUniqueLock<UE::FMutex> lock(ThreadStreamsMutex);
			ThreadStreams.Add(stream);
		}
		Sink->OnInitThreadStream(stream);
	}

	template <typename T>
	void QueueLogEntry(const T& event)
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		dispatch->LogMutex.Lock();
		dispatch->LogEntries->GetCurrentBlock().GetEvents().Push(event);
		if (dispatch->LogEntries->IsFull())
		{
			dispatch->FlushLogStreamImpl(dispatch->LogMutex); // unlocks the mutex
		}
		else
		{
			dispatch->LogMutex.Unlock();
		}
	}

	void FlushLogStream()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		dispatch->LogMutex.Lock();
		dispatch->FlushLogStreamImpl(dispatch->LogMutex); // unlocks the mutex
	}

	void FlushMetricStream()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		dispatch->MetricMutex.Lock();
		dispatch->FlushMetricStreamImpl(dispatch->MetricMutex); // unlocks the mutex
	}

	void Shutdown()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		dispatch->Sink->OnShutdown();
		GDispatch = nullptr;
	}

	void LogInterop(const LogStringInteropEvent& event)
	{
		QueueLogEntry(event);
	}

	void LogStaticStr(const LogStaticStrEvent& event)
	{
		QueueLogEntry(event);
	}

	template <typename T>
	void QueueMetric(const T& event)
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		dispatch->MetricMutex.Lock();
		dispatch->Metrics->GetCurrentBlock().GetEvents().Push(event);
		if (dispatch->Metrics->IsFull())
		{
			dispatch->FlushMetricStreamImpl(dispatch->MetricMutex); // unlocks the mutex
		}
		else
		{
			dispatch->MetricMutex.Unlock();
		}
	}

	void IntMetric(const IntegerMetricEvent& event)
	{
		QueueMetric(event);
	}

	void FloatMetric(const FloatMetricEvent& event)
	{
		QueueMetric(event);
	}

	ThreadStream* GetCurrentThreadStream()
	{
		thread_local ThreadStream* ptr = nullptr;
		if (ptr)
		{
			return ptr;
		}
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return nullptr;
		}
		ptr = dispatch->AllocThreadStream();
		dispatch->PublishThreadStream(ptr);
		return ptr;
	}

	template <typename T>
	void QueueThreadEvent(const T& event)
	{
		if (ThreadStream* stream = GetCurrentThreadStream())
		{
			stream->GetCurrentBlock().GetEvents().Push(event);
			if (stream->IsFull())
			{
				Dispatch* dispatch = GDispatch;
				if (!dispatch)
				{
					return;
				}
				dispatch->FlushThreadStream(stream);
			}
		}
	}

	void BeginScope(const BeginThreadSpanEvent& event)
	{
		QueueThreadEvent(event);
	}

	void EndScope(const EndThreadSpanEvent& event)
	{
		QueueThreadEvent(event);
	}

	void BeginNamedSpan(const BeginThreadNamedSpanEvent& event)
	{
		QueueThreadEvent(event);
	}

	void EndNamedSpan(const EndThreadNamedSpanEvent& event)
	{
		QueueThreadEvent(event);
	}

	void ForEachThreadStream(ThreadStreamCallback callback)
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		UE::TUniqueLock<UE::FMutex> lock(dispatch->ThreadStreamsMutex);
		for (ThreadStream* stream : dispatch->ThreadStreams)
		{
			callback(stream);
		}
	}

	void FlushCurrentThreadStream()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		ThreadStream* stream = GetCurrentThreadStream();
		if (!stream)
		{
			return;
		}
		dispatch->FlushThreadStream(stream);
	}

} // namespace MicromegasTracing
