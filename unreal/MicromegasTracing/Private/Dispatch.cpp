//
//  MicromegasTracing/Dispatch.cpp
//
#include "MicromegasTracing/Dispatch.h"
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
		const std::shared_ptr<EventSink>& sink,
		const ProcessInfoPtr& processInfo,
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
		LogBlockPtr logBlock = std::make_shared<LogBlock>(logStreamId,
			processInfo->StartTime,
			LogBufferSize,
			0);
		LogEntries = std::make_shared<LogStream>(CurrentProcessInfo->ProcessId,
			logStreamId,
			logBlock,
			TArray<FString>({ TEXT("log") }));

		FString metricStreamId = allocNewGuid();
		MetricsBlockPtr metricBlock = std::make_shared<MetricBlock>(metricStreamId,
			processInfo->StartTime,
			metricBufferSize,
			0);
		Metrics = std::make_shared<MetricStream>(CurrentProcessInfo->ProcessId,
			metricStreamId,
			metricBlock,
			TArray<FString>({ TEXT("metrics") }));
	}

	Dispatch::~Dispatch()
	{
	}

	void Dispatch::Init(NewGuid allocNewGuid,
		const ProcessInfoPtr& processInfo,
		const std::shared_ptr<EventSink>& sink,
		size_t logBufferSize,
		size_t metricBufferSize,
		size_t threadBufferSize)
	{
		if (GDispatch)
		{
			return;
		}
		GDispatch = new Dispatch(allocNewGuid, sink, processInfo, logBufferSize, metricBufferSize, threadBufferSize);
		sink->OnStartup(processInfo);
		sink->OnInitLogStream(GDispatch->LogEntries);
		sink->OnInitMetricStream(GDispatch->Metrics);
	}

	void Dispatch::FlushLogStreamImpl(GuardPtr& guard)
	{
		MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTracing"), TEXT("Dispatch::FlushLogStreamImpl"));
		if (LogEntries->GetCurrentBlock().IsEmpty())
		{
			return;
		}
		DualTime now = DualTime::Now();
		size_t new_offset = LogEntries->GetCurrentBlock().GetOffset() + LogEntries->GetCurrentBlock().GetEvents().GetNbEvents();
		LogBlockPtr newBlock = std::make_shared<LogBlock>(LogEntries->GetStreamId(),
			now,
			LogBufferSize,
			new_offset);
		LogBlockPtr fullBlock = LogEntries->SwapBlocks(newBlock);
		fullBlock->Close(now);
		guard.reset();
		Sink->OnProcessLogBlock(fullBlock);
	}

	void Dispatch::FlushMetricStreamImpl(GuardPtr& guard)
	{
		MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTracing"), TEXT("Dispatch::FlushMetricStreamImpl"));
		if (Metrics->GetCurrentBlock().IsEmpty())
		{
			return;
		}
		DualTime now = DualTime::Now();
		size_t new_offset = Metrics->GetCurrentBlock().GetOffset() + Metrics->GetCurrentBlock().GetEvents().GetNbEvents();
		MetricsBlockPtr newBlock = std::make_shared<MetricBlock>(Metrics->GetStreamId(),
			now,
			MetricBufferSize,
			new_offset);
		MetricsBlockPtr fullBlock = Metrics->SwapBlocks(newBlock);
		fullBlock->Close(now);
		guard.reset();
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
		ThreadBlockPtr newBlock = std::make_shared<ThreadBlock>(stream->GetStreamId(),
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
		ThreadBlockPtr block = std::make_shared<ThreadBlock>(streamId,
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
			std::lock_guard<std::recursive_mutex> guard(ThreadStreamsMutex);
			ThreadStreams.push_back(stream);
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
		auto guard = std::make_unique<std::lock_guard<std::recursive_mutex>>(dispatch->LogMutex);
		dispatch->LogEntries->GetCurrentBlock().GetEvents().Push(event);
		if (dispatch->LogEntries->IsFull())
		{
			dispatch->FlushLogStreamImpl(guard); // unlocks the mutex
		}
	}

	void FlushLogStream()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		auto guard = std::make_unique<std::lock_guard<std::recursive_mutex>>(dispatch->LogMutex);
		dispatch->FlushLogStreamImpl(guard); // unlocks the mutex
	}

	void FlushMetricStream()
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		auto guard = std::make_unique<std::lock_guard<std::recursive_mutex>>(dispatch->MetricMutex);
		dispatch->FlushMetricStreamImpl(guard); // unlocks the mutex
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
		auto guard = std::make_unique<std::lock_guard<std::recursive_mutex>>(dispatch->MetricMutex);
		dispatch->Metrics->GetCurrentBlock().GetEvents().Push(event);
		if (dispatch->Metrics->IsFull())
		{
			dispatch->FlushMetricStreamImpl(guard); // unlocks the mutex
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

	void ForEachThreadStream(ThreadStreamCallback callback)
	{
		Dispatch* dispatch = GDispatch;
		if (!dispatch)
		{
			return;
		}
		std::lock_guard<std::recursive_mutex> guard(dispatch->ThreadStreamsMutex);
		for (ThreadStream* stream : dispatch->ThreadStreams)
		{
			callback(stream);
		}
	}

} // namespace MicromegasTracing
