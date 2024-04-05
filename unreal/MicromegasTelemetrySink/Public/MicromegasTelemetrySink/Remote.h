#pragma once
//
//  MicromegasTelemetrySink/Remote.h
//
#include <functional>
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"

class FlushMonitor;

class RemoteSink : public MicromegasTracing::EventSink, public FRunnable
{
public:
	RemoteSink(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth);
	virtual ~RemoteSink();

	//
	//  MicromegasTracing::EventSink
	//
	virtual void OnStartup(const MicromegasTracing::ProcessInfoPtr& processInfo);
	virtual void OnShutdown();
	virtual void OnInitLogStream(const MicromegasTracing::LogStreamPtr& stream);
	virtual void OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& stream);
	virtual void OnInitThreadStream(MicromegasTracing::ThreadStream* stream);
	virtual void OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& block);
	virtual void OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& block);
	virtual void OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& block);
	virtual bool IsBusy();

	//
	//  FRunnable
	//
	virtual uint32 Run();

private:
	void IncrementQueueSize();
	void SendJsonRequest(const TCHAR* command, const FString& content);
	void SendBinaryRequest(const TCHAR* command, const TArray<uint8>& content);

	typedef std::function<void()> Callback;
	typedef TQueue<Callback, EQueueMode::Mpsc> WorkQueue;
	FString BaseUrl;
	SharedTelemetryAuthenticator Auth;
	WorkQueue Queue;
	volatile int32 QueueSize;
	volatile bool RequestShutdown;
	FEventRef WakeupThread;
	TUniquePtr<FRunnableThread> Thread;
	TUniquePtr<FlushMonitor> Flusher;
};

MICROMEGASTELEMETRYSINK_API void InitRemoteSink(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth);
