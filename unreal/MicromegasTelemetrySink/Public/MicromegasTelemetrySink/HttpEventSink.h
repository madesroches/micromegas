#pragma once
//
//  MicromegasTelemetrySink/HttpEventSink.h
//
#include "Containers/Map.h"
#include "Containers/Queue.h"
#include "Containers/UnrealString.h"
#include "HAL/Event.h"
#include "HAL/Runnable.h"
#include "MicromegasTelemetrySink/FlushMonitor.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/Fwd.h"
#include "SamplingController.h"
#include "Templates/SharedPointer.h"
#include <atomic>
#include <functional>

class FlushMonitor;

// HTTP private dependency — full definition in HttpEventSink.cpp
namespace FHttpRetrySystem
{
	class FManager;
}

enum class EUploadPriority : uint8
{
	Metadata = 0,
	Logs     = 1,
	Metrics  = 2,
	Traces   = 3,
};

struct FCompletionState
{
	std::atomic<int64> HttpInFlightRequests{ 0 };
	FEventRef          WakeupThread;
};

struct FQueuedWork
{
	std::function<void()> Work;
	int32                 PayloadBytes = 0;

	FQueuedWork() = default;
	FQueuedWork(std::function<void()> InWork, int32 InPayloadBytes)
		: Work(MoveTemp(InWork))
		, PayloadBytes(InPayloadBytes)
	{}
};

class HttpEventSink : public MicromegasTracing::EventSink, public FRunnable
{
public:
	HttpEventSink(const FString& BaseUrl,
		const MicromegasTracing::ProcessInfoPtr& ThisProcess,
		const SharedTelemetryAuthenticator& InAuth,
		const SharedSamplingController& InSampling,
		const SharedFlushMonitor& InFlusher);
	virtual ~HttpEventSink();

	//
	//  MicromegasTracing::EventSink
	//
	virtual void OnStartup(const MicromegasTracing::ProcessInfoPtr& ProcessInfo) override;
	virtual void OnShutdown() override;
	virtual void OnInitLogStream(const MicromegasTracing::LogStreamPtr& Stream) override;
	virtual void OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& Stream) override;
	virtual void OnInitThreadStream(MicromegasTracing::ThreadStream* Stream) override;
	virtual void OnInitNetStream(const MicromegasTracing::NetStreamPtr& Stream) override;
	virtual void OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& Block) override;
	virtual void OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& Block) override;
	virtual void OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& Block) override;
	virtual void OnProcessNetBlock(const MicromegasTracing::NetBlockPtr& Block) override;
	virtual bool IsBusy() override;
	virtual void OnAuthUpdated() override;

	//
	//  FRunnable
	//
	virtual uint32 Run() override;

private:
	typedef std::function<void()> Callback;
	typedef TQueue<FQueuedWork, EQueueMode::Mpsc> WorkQueue;

	void IncrementQueueSize();
	void DecrementQueueSize();
	bool DrainOneItem();
	void EnqueueWithPriority(EUploadPriority Priority, int32 PayloadBytes, Callback Work);
	WorkQueue& QueueForPriority(EUploadPriority UploadPriority);
	void SendBinaryRequest(const TCHAR* Command, const TArray<uint8>& Content, EUploadPriority Priority);

	template <typename BlockPtrT>
	void EnqueueBlock(const BlockPtrT& Block, EUploadPriority Priority);

	void EnqueueStreamInit(
		EUploadPriority Priority,
		TFunction<TArray<uint8>()> FormatFn);

	FString BaseUrl;
	MicromegasTracing::ProcessInfoPtr Process;
	SharedTelemetryAuthenticator Auth;
	SharedSamplingController Sampling;

	WorkQueue MetadataQueue;
	WorkQueue LogQueue;
	WorkQueue MetricQueue;
	WorkQueue TraceQueue;

	std::atomic<int32> QueueSize{ 0 };
	std::atomic<int64> QueueSizeBytes{ 0 };
	std::atomic<int64> DroppedUploads{ 0 };

	std::atomic<bool> RequestShutdown{ false };
	TUniquePtr<FRunnableThread> Thread;
	SharedFlushMonitor Flusher;

	TSharedPtr<FCompletionState, ESPMode::ThreadSafe> State;
	TSharedPtr<FHttpRetrySystem::FManager> RetryManager;
};

MICROMEGASTELEMETRYSINK_API TSharedPtr<MicromegasTracing::EventSink> InitHttpEventSink(
	const FString& BaseUrl,
	const SharedTelemetryAuthenticator& Auth,
	const SharedSamplingController& Sampling,
	const SharedFlushMonitor& Flusher,
	const TMap<FString,FString>& AdditionalProcessProperties);
