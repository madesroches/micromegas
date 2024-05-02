#pragma once
//
//  MicromegasTelemetrySink/HttpEventSink.h
//
#include "Containers/Queue.h"
#include "Containers/UnrealString.h"
#include "HAL/Event.h"
#include "HAL/Runnable.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/Fwd.h"
#include "Templates/SharedPointer.h"
#include <functional>

class FlushMonitor;

class HttpEventSink : public MicromegasTracing::EventSink, public FRunnable
{
public:
	HttpEventSink(const FString& BaseUrl, const MicromegasTracing::ProcessInfoPtr& ThisProcess, const SharedTelemetryAuthenticator& Auth);
	virtual ~HttpEventSink();

	//
	//  MicromegasTracing::EventSink
	//
	virtual void OnStartup(const MicromegasTracing::ProcessInfoPtr& processInfo) override;
	virtual void OnShutdown() override;
	virtual void OnInitLogStream(const MicromegasTracing::LogStreamPtr& stream) override;
	virtual void OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& stream) override;
	virtual void OnInitThreadStream(MicromegasTracing::ThreadStream* stream) override;
	virtual void OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& block) override;
	virtual void OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& block) override;
	virtual void OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& block) override;
	virtual bool IsBusy() override;
	virtual void OnAuthUpdated() override;

	//
	//  FRunnable
	//
	virtual uint32 Run();

private:
	void IncrementQueueSize();
	void SendBinaryRequest(const TCHAR* command, const TArray<uint8>& content, float TimeoutSeconds);

	typedef std::function<void()> Callback;
	typedef TQueue<Callback, EQueueMode::Mpsc> WorkQueue;
	FString BaseUrl;
	MicromegasTracing::ProcessInfoPtr Process;
	SharedTelemetryAuthenticator Auth;
	WorkQueue Queue;
	volatile int32 QueueSize;
	volatile bool RequestShutdown;
	FEventRef WakeupThread;
	TUniquePtr<FRunnableThread> Thread;
	TUniquePtr<FlushMonitor> Flusher;
};

MICROMEGASTELEMETRYSINK_API TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> InitHttpEventSink(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth);
