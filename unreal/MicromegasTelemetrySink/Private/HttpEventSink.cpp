#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "HAL/PlatformProcess.h"
#include "HAL/Runnable.h"
#include "HAL/RunnableThread.h"
#include "HAL/ThreadManager.h"
#include "HttpModule.h"
#include "InsertBlockRequest.h"
#include "InsertProcessRequest.h"
#include "InsertStreamRequest.h"
#include "Interfaces/IHttpResponse.h"
#include "LogDependencies.h"
#include "MicromegasTelemetrySink/FlushMonitor.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/EventStream.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "Misc/App.h"
#include <sstream>
#include <string>
#if PLATFORM_WINDOWS
	#include "Windows/WindowsSystemIncludes.h"
	#include "Windows/WindowsHWrapper.h"
#endif

DEFINE_LOG_CATEGORY(LogMicromegasTelemetrySink);

namespace
{
	void OnProcessRequestComplete(FHttpRequestPtr HttpRequest, FHttpResponsePtr HttpResponse, bool bSucceeded)
	{
		int32 Code = HttpResponse ? HttpResponse->GetResponseCode() : 0;
		if (!bSucceeded || Code != 200)
		{
			UE_LOG(LogMicromegasTelemetrySink, Error, TEXT("Request completed with code=%d response=%s"), Code, HttpResponse ? *(HttpResponse->GetContentAsString()) : TEXT(""));
		}
	}

	uint64 GetTscFrequency()
	{
#if PLATFORM_WINDOWS
		LARGE_INTEGER Frequency;
		verify(QueryPerformanceFrequency(&Frequency));
		return Frequency.QuadPart;
#else
		return static_cast<uint64>(1.0 / FPlatformTime::GetSecondsPerCycle64());
#endif
	}

} // namespace

HttpEventSink::HttpEventSink(const FString& baseUrl, const MicromegasTracing::ProcessInfoPtr& ThisProcess, const SharedTelemetryAuthenticator& Auth)
	: BaseUrl(baseUrl)
	, Process(ThisProcess)
	, Auth(Auth)
	, QueueSize(0)
	, RequestShutdown(false)
{
	Thread.Reset(FRunnableThread::Create(this, TEXT("MicromegasHttpTelemetrySink")));
	Flusher.Reset(new FlushMonitor());
}

HttpEventSink::~HttpEventSink()
{
}

void HttpEventSink::OnAuthUpdated()
{
	WakeupThread->Trigger();
}

void HttpEventSink::OnStartup(const MicromegasTracing::ProcessInfoPtr& processInfo)
{
	FPlatformAtomics::InterlockedIncrement(&QueueSize);
	Queue.Enqueue([this, processInfo]() {
		TArray<uint8> body = FormatInsertProcessRequest(*processInfo);
		const float TimeoutSeconds = 30.0f;
		SendBinaryRequest(TEXT("insert_process"), body, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnShutdown()
{
	MICROMEGAS_LOG_STATIC(TEXT("MicromegasTelemetrySink"), MicromegasTracing::LogLevel::Info, TEXT("Shutting down"));
	Flusher.Reset();
	MicromegasTracing::FlushLogStream();
	MicromegasTracing::FlushMetricStream();
	RequestShutdown = true;
	WakeupThread->Trigger();
	Thread->WaitForCompletion();
}

void HttpEventSink::OnInitLogStream(const MicromegasTracing::LogStreamPtr& stream)
{
	IncrementQueueSize();
	Queue.Enqueue([this, stream]() {
		TArray<uint8> body = FormatInsertLogStreamRequest(*stream);
		const float TimeoutSeconds = 30.0f;
		SendBinaryRequest(TEXT("insert_stream"), body, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& stream)
{
	IncrementQueueSize();
	Queue.Enqueue([this, stream]() {
		TArray<uint8> body = FormatInsertMetricStreamRequest(*stream);
		const float TimeoutSeconds = 30.0f;
		SendBinaryRequest(TEXT("insert_stream"), body, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnInitThreadStream(MicromegasTracing::ThreadStream* stream)
{
	const uint32 threadId = FPlatformTLS::GetCurrentThreadId();
	const FString& threadName = FThreadManager::GetThreadName(threadId);

	stream->SetProperty(TEXT("thread-name"), *threadName);
	stream->SetProperty(TEXT("thread-id"), *FString::Format(TEXT("{0}"), { threadId }));

	IncrementQueueSize();
	Queue.Enqueue([this, stream]() {
		TArray<uint8> body = FormatInsertThreadStreamRequest(*stream);
		const float TimeoutSeconds = 30.0f;
		SendBinaryRequest(TEXT("insert_stream"), body, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& block)
{
	IncrementQueueSize();
	Queue.Enqueue([this, block]() {
		TArray<uint8> content = FormatBlockRequest(*Process, *block);
		const float TimeoutSeconds = 10.0f;
		SendBinaryRequest(TEXT("insert_block"), content, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& block)
{
	IncrementQueueSize();
	Queue.Enqueue([this, block]() {
		TArray<uint8> content = FormatBlockRequest(*Process, *block);
		const float TimeoutSeconds = 10.0f;
		SendBinaryRequest(TEXT("insert_block"), content, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& block)
{
	MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("OnProcessThreadBlock"));
	IncrementQueueSize();
	Queue.Enqueue([this, block]() {
		TArray<uint8> content = FormatBlockRequest(*Process, *block);
		const float TimeoutSeconds = 2.0f;
		SendBinaryRequest(TEXT("insert_block"), content, TimeoutSeconds);
	});
	WakeupThread->Trigger();
}

bool HttpEventSink::IsBusy()
{
	return QueueSize > 0;
}

uint32 HttpEventSink::Run()
{
	while (true)
	{
		if (Auth->IsReady())
		{
			Callback c;
			while (Queue.Dequeue(c))
			{
				int32 newQueueSize = FPlatformAtomics::InterlockedDecrement(&QueueSize);
				MICROMEGAS_IMETRIC(TEXT("MicromegasTelemetrySink"), MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), newQueueSize);
				c();
			}
		}

		if (RequestShutdown)
		{
			break;
		}
		const uint32 timeout_ms = 60 * 1000;
		WakeupThread->Wait(timeout_ms);
		Flusher->Tick(this);
	}
	return 0;
}

void HttpEventSink::IncrementQueueSize()
{
	MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("IncrementQueueSize"));
	int32 incrementedQueueSize = FPlatformAtomics::InterlockedIncrement(&QueueSize);
	MICROMEGAS_IMETRIC(TEXT("MicromegasTelemetrySink"), MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), incrementedQueueSize);
}

void HttpEventSink::SendBinaryRequest(const TCHAR* command, const TArray<uint8>& content, float TimeoutSeconds)
{
	MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("SendBinaryRequest"));
	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> HttpRequest = FHttpModule::Get().CreateRequest();
	HttpRequest->SetURL(BaseUrl + command);
	HttpRequest->SetVerb(TEXT("POST"));
	HttpRequest->SetContent(content);
	HttpRequest->SetHeader(TEXT("Content-Type"), TEXT("application/octet-stream"));
	HttpRequest->SetTimeout(TimeoutSeconds);
	HttpRequest->SetDelegateThreadPolicy(EHttpRequestDelegateThreadPolicy::CompleteOnHttpThread);
	HttpRequest->OnProcessRequestComplete().BindStatic(&OnProcessRequestComplete);
	if (!Auth->Sign(*HttpRequest))
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to sign telemetry http request"));
		return;
	}

	if (!HttpRequest->ProcessRequest())
	{
		UE_LOG(LogMicromegasTelemetrySink, Error, TEXT("Failed to initialize telemetry http request"));
	}
}

FString CreateGuid()
{
	return FGuid::NewGuid().ToString(EGuidFormats::DigitsWithHyphens);
}

FString GetDistro()
{
	return FString::Printf(TEXT("%s %s"), ANSI_TO_TCHAR(FPlatformProperties::PlatformName()), *FPlatformMisc::GetOSVersion());
}

TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> InitHttpEventSink(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth)
{
	using namespace MicromegasTracing;
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Initializing Remote Telemetry Sink"));

	DualTime StartTime = DualTime::Now();
	FString ProcessId = CreateGuid();
	FString ParentProcessId = FPlatformMisc::GetEnvironmentVariable(TEXT("MICROMEGAS_TELEMETRY_PARENT_PROCESS"));
	FPlatformMisc::SetEnvironmentVar(TEXT("MICROMEGAS_TELEMETRY_PARENT_PROCESS"), *ProcessId);

	ProcessInfoPtr Process(new ProcessInfo());
	Process->ProcessId = ProcessId;
	Process->ParentProcessId = ParentProcessId;
	Process->Exe = FPlatformProcess::ExecutablePath();
	Process->Username = FPlatformProcess::UserName(false);
	Process->Computer = FPlatformProcess::ComputerName();
	Process->Distro = GetDistro();
	Process->CpuBrand = *FPlatformMisc::GetCPUBrand();
	Process->TscFrequency = GetTscFrequency();
	Process->StartTime = StartTime;
	Process->Properties.Add(TEXT("build-version"), FApp::GetBuildVersion());

	TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink = MakeShared<HttpEventSink>(BaseUrl, Process, Auth);
	const size_t LOG_BUFFER_SIZE = 10 * 1024 * 1024;
	const size_t METRICS_BUFFER_SIZE = 10 * 1024 * 1024;
	const size_t THREAD_BUFFER_SIZE = 10 * 1024 * 1024;

	Dispatch::Init(&CreateGuid, Process, Sink, LOG_BUFFER_SIZE, METRICS_BUFFER_SIZE, THREAD_BUFFER_SIZE);
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Initializing Legion Telemetry for process %s"), *Process->ProcessId);
	MICROMEGAS_LOG_STATIC(TEXT("MicromegasTelemetrySink"), LogLevel::Info, TEXT("Telemetry enabled"));
	return Sink;
}
