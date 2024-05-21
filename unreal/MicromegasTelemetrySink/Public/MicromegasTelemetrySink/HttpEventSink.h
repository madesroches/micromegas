#pragma once
//
//  MicromegasTelemetrySink/HttpEventSink.h
//
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
#include <functional>

class FlushMonitor;

class HttpEventSink : public MicromegasTracing::EventSink, public FRunnable
{
public:
	HttpEventSink(const FString& BaseUrl,
		const MicromegasTracing::ProcessInfoPtr& ThisProcess,
		const SharedTelemetryAuthenticator& InAuth,
		const SharedSampingController& InSampling,
		const FlushMonitorPtr& InFlusher);
	virtual ~HttpEventSink();

	//
	//  MicromegasTracing::EventSink
	//
	virtual void OnStartup(const MicromegasTracing::ProcessInfoPtr& ProcessInfo) override;
	virtual void OnShutdown() override;
	virtual void OnInitLogStream(const MicromegasTracing::LogStreamPtr& Stream) override;
	virtual void OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& Stream) override;
	virtual void OnInitThreadStream(MicromegasTracing::ThreadStream* Stream) override;
	virtual void OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& Block) override;
	virtual void OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& Block) override;
	virtual void OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& Block) override;
	virtual bool IsBusy() override;
	virtual void OnAuthUpdated() override;

	//
	//  FRunnable
	//
	virtual uint32 Run();

private:
	void IncrementQueueSize();
	void SendBinaryRequest(const TCHAR* Command, const TArray<uint8>& Content, float TimeoutSeconds);

	typedef std::function<void()> Callback;
	typedef TQueue<Callback, EQueueMode::Mpsc> WorkQueue;
	FString BaseUrl;
	MicromegasTracing::ProcessInfoPtr Process;
	SharedTelemetryAuthenticator Auth;
	SharedSampingController Sampling;
	WorkQueue Queue;
	volatile int32 QueueSize;
	volatile bool RequestShutdown;
	FEventRef WakeupThread;
	TUniquePtr<FRunnableThread> Thread;
	FlushMonitorPtr Flusher;
};

MICROMEGASTELEMETRYSINK_API TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> InitHttpEventSink(
	const FString& BaseUrl,
	const SharedTelemetryAuthenticator& Auth,
	const SharedSampingController& Sampling,
	const FlushMonitorPtr& Flusher);
