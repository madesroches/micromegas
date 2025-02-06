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
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/EventStream.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "Misc/App.h"
#include "BuildSettings.h"
#include "Misc/EngineVersion.h"
#include <sstream>
#include <string>
#if PLATFORM_WINDOWS
	#include "Windows/WindowsSystemIncludes.h"
	#include "Windows/WindowsHWrapper.h"
#endif

DEFINE_LOG_CATEGORY(LogMicromegasTelemetrySink);

namespace
{
	void RetryRequest(FHttpRequestPtr OldRequest, int RemainingRetries);

	void OnProcessRequestComplete(FHttpRequestPtr HttpRequest, FHttpResponsePtr HttpResponse, bool bSucceeded, int RemainingRetries, uint64 StartTimestamp)
	{
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpRequestCompletionTime"), TEXT("ticks"), FPlatformTime::Cycles64() - StartTimestamp);
		if (!HttpResponse.IsValid() && RemainingRetries > 0)
		{
			// the most common error we see is not from the server reporting an error, it's an internal client error failing to rewind a request
			// in this case, we get a null HttpResponse
			RetryRequest(HttpRequest, RemainingRetries - 1);
			return;
		}
		int32 Code = HttpResponse ? HttpResponse->GetResponseCode() : 0;
		if (!bSucceeded || Code != 200)
		{
			UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Request completed with code=%d response=%s"), Code, HttpResponse ? *(HttpResponse->GetContentAsString()) : TEXT(""));
		}
	}

	void RetryRequest(FHttpRequestPtr OldRequest, int RemainingRetries)
	{
		UE_LOG(LogMicromegasTelemetrySink, Verbose, TEXT("Retrying telemetry http request, RemainingRetries=%d"), RemainingRetries);
		// can't reuse the old request, need to clone it
		TSharedRef<IHttpRequest, ESPMode::ThreadSafe> NewRequest = FHttpModule::Get().CreateRequest();
		NewRequest->SetURL(OldRequest->GetURL());
		NewRequest->SetVerb(OldRequest->GetVerb());
		NewRequest->SetContent(OldRequest->GetContent());
		for (FString HeaderPair : OldRequest->GetAllHeaders())
		{
			FString Name;
			FString Value;
			if (!HeaderPair.Split(TEXT(":"), &Name, &Value))
			{
				UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Malformed header pair %s"), *HeaderPair);
				return;
			}
			NewRequest->SetHeader(Name, Value);
		}
		TOptional<float> Timeout = OldRequest->GetTimeout();
		if (Timeout.IsSet())
		{
			NewRequest->SetTimeout(Timeout.GetValue());
		}
		NewRequest->SetDelegateThreadPolicy(OldRequest->GetDelegateThreadPolicy());
		uint64 StartTimestamp = FPlatformTime::Cycles64();
		NewRequest->OnProcessRequestComplete().BindLambda([RemainingRetries, StartTimestamp](FHttpRequestPtr HttpRequest, FHttpResponsePtr HttpResponse, bool bSucceeded)
			{
				OnProcessRequestComplete(HttpRequest, HttpResponse, bSucceeded, RemainingRetries, StartTimestamp);
			});
		if (!NewRequest->ProcessRequest())
		{
			UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to retry telemetry http request"));
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

HttpEventSink::HttpEventSink(const FString& InBaseUrl,
	const MicromegasTracing::ProcessInfoPtr& ThisProcess,
	const SharedTelemetryAuthenticator& InAuth,
	const SharedSamplingController& InSampling,
	const SharedFlushMonitor& InFlusher)
	: BaseUrl(InBaseUrl)
	, Process(ThisProcess)
	, Auth(InAuth)
	, Sampling(InSampling)
	, QueueSize(0)
	, RequestShutdown(false)
	, Flusher(InFlusher)
{
	Thread.Reset(FRunnableThread::Create(this, TEXT("MicromegasHttpTelemetrySink")));
}

HttpEventSink::~HttpEventSink()
{
}

void HttpEventSink::OnAuthUpdated()
{
	WakeupThread->Trigger();
}

void HttpEventSink::OnStartup(const MicromegasTracing::ProcessInfoPtr& ProcessInfo)
{
	MicromegasTracing::Dispatch::InitCurrentThreadStream();
	IncrementQueueSize();
	Queue.Enqueue([this, ProcessInfo]()
		{
			TArray<uint8> Body = FormatInsertProcessRequest(*ProcessInfo);
			const float TimeoutSeconds = 30.0f;
			SendBinaryRequest(TEXT("insert_process"), Body, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnShutdown()
{
	MICROMEGAS_LOG("MicromegasTelemetrySink", MicromegasTracing::LogLevel::Info, TEXT("Shutting down"));
	RequestShutdown = true;
	WakeupThread->Trigger();
	Thread->WaitForCompletion();
}

void HttpEventSink::OnInitLogStream(const MicromegasTracing::LogStreamPtr& Stream)
{
	IncrementQueueSize();
	Queue.Enqueue([this, Stream]()
		{
			TArray<uint8> Body = FormatInsertLogStreamRequest(*Stream);
			const float TimeoutSeconds = 30.0f;
			SendBinaryRequest(TEXT("insert_stream"), Body, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& Stream)
{
	IncrementQueueSize();
	Queue.Enqueue([this, Stream]()
		{
			TArray<uint8> Body = FormatInsertMetricStreamRequest(*Stream);
			const float TimeoutSeconds = 30.0f;
			SendBinaryRequest(TEXT("insert_stream"), Body, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnInitThreadStream(MicromegasTracing::ThreadStream* Stream)
{
	const uint32 ThreadId = FPlatformTLS::GetCurrentThreadId();
	const FString& ThreadName = FThreadManager::GetThreadName(ThreadId);

	Stream->SetProperty(TEXT("thread-name"), *ThreadName);
	Stream->SetProperty(TEXT("thread-id"), *FString::Format(TEXT("{0}"), { ThreadId }));

	IncrementQueueSize();
	Queue.Enqueue([this, Stream]()
		{
			TArray<uint8> Body = FormatInsertThreadStreamRequest(*Stream);
			const float TimeoutSeconds = 30.0f;
			SendBinaryRequest(TEXT("insert_stream"), Body, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& Block)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (!Sampling->ShouldSampleBlock(Block))
	{
		return;
	}
	IncrementQueueSize();
	Queue.Enqueue([this, Block]()
		{
			TArray<uint8> Content = FormatBlockRequest(*Process, *Block);
			const float TimeoutSeconds = 10.0f;
			SendBinaryRequest(TEXT("insert_block"), Content, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& Block)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (!Sampling->ShouldSampleBlock(Block))
	{
		return;
	}
	IncrementQueueSize();
	Queue.Enqueue([this, Block]()
		{
			TArray<uint8> Content = FormatBlockRequest(*Process, *Block);
			const float TimeoutSeconds = 10.0f;
			SendBinaryRequest(TEXT("insert_block"), Content, TimeoutSeconds);
		});
	WakeupThread->Trigger();
}

void HttpEventSink::OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& Block)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (!Sampling->ShouldSampleBlock(Block))
	{
		return;
	}
	IncrementQueueSize();
	Queue.Enqueue([this, Block]()
		{
			TArray<uint8> Content = FormatBlockRequest(*Process, *Block);
			const float TimeoutSeconds = 2.0f;
			SendBinaryRequest(TEXT("insert_block"), Content, TimeoutSeconds);
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
				MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), newQueueSize);
				c();
			}
		}

		if (RequestShutdown)
		{
			break;
		}
		const uint32 TimeoutMs = static_cast<uint32>(Flusher->Tick(this) * 1000.0);
		WakeupThread->Wait(TimeoutMs);
	}
	return 0;
}

void HttpEventSink::IncrementQueueSize()
{
	int32 incrementedQueueSize = FPlatformAtomics::InterlockedIncrement(&QueueSize);
	MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), incrementedQueueSize);
}

void HttpEventSink::SendBinaryRequest(const TCHAR* command, const TArray<uint8>& content, float TimeoutSeconds)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> HttpRequest = FHttpModule::Get().CreateRequest();
	HttpRequest->SetURL(BaseUrl + command);
	HttpRequest->SetVerb(TEXT("POST"));
	HttpRequest->SetContent(content);
	HttpRequest->SetHeader(TEXT("Content-Type"), TEXT("application/octet-stream"));
	HttpRequest->SetTimeout(TimeoutSeconds);
	HttpRequest->SetDelegateThreadPolicy(EHttpRequestDelegateThreadPolicy::CompleteOnHttpThread);
	uint64 StartTimestamp = FPlatformTime::Cycles64();
	HttpRequest->OnProcessRequestComplete().BindLambda([StartTimestamp](FHttpRequestPtr HttpRequest, FHttpResponsePtr HttpResponse, bool bSucceeded)
		{
			const int RetryCount = 1;
			OnProcessRequestComplete(HttpRequest, HttpResponse, bSucceeded, RetryCount, StartTimestamp);
		});
	if (!Auth->Sign(*HttpRequest))
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to sign telemetry http request"));
		return;
	}

	if (!HttpRequest->ProcessRequest())
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to initialize telemetry http request"));
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

TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> InitHttpEventSink(
	const FString& BaseUrl,
	const SharedTelemetryAuthenticator& Auth,
	const SharedSamplingController& Sampling,
	const SharedFlushMonitor& Flusher)
{
	using namespace MicromegasTracing;
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Initializing Remote Telemetry Sink"));

	DualTime StartTime = DualTime::Now();
	FString ProcessId = CreateGuid();
	FString ParentProcessId = FPlatformMisc::GetEnvironmentVariable(TEXT("MICROMEGAS_TELEMETRY_PARENT_PROCESS"));
#if UE_EDITOR
	// this logs an error in console builds, the game should not be spawning processes anyway
	FPlatformMisc::SetEnvironmentVar(TEXT("MICROMEGAS_TELEMETRY_PARENT_PROCESS"), *ProcessId);
#endif

	ProcessInfoPtr Process(new ProcessInfo());
	Process->ProcessId = ProcessId;
	Process->ParentProcessId = ParentProcessId;
	Process->Exe = FPlatformProcess::ExecutableName(false);
	if (Process->Exe.IsEmpty())
	{
		Process->Exe = FApp::GetProjectName();
	}
	if (Process->Exe.IsEmpty())
	{
  		Process->Exe = TEXT("UnrealEngine");
	}
	Process->Username = FPlatformProcess::UserName(false);
	Process->Computer = FPlatformProcess::ComputerName();
	Process->Distro = GetDistro();
	Process->CpuBrand = *FPlatformMisc::GetCPUBrand();
	Process->TscFrequency = GetTscFrequency();
	Process->StartTime = StartTime;

	// Currently this data duplicates some of the data in the process info, but the goal is to move it here
	// and leave the process info with only the minimum necessary
	Process->Properties.Add(TEXT("platform-name"), FPlatformProperties::IniPlatformName());
	Process->Properties.Add(TEXT("build-version"), FApp::GetBuildVersion());
	Process->Properties.Add(TEXT("engine-version"), *FEngineVersion::Current().ToString());
	Process->Properties.Add(TEXT("build-config"), LexToString(FApp::GetBuildConfiguration()));
	Process->Properties.Add(TEXT("build-target"), LexToString(FApp::GetBuildTargetType()));
	Process->Properties.Add(TEXT("branch-name"), FApp::GetBranchName().ToLower());
	Process->Properties.Add(TEXT("commit"), FString::FromInt(BuildSettings::GetCurrentChangelist()));
	//Process->Properties.Add(TEXT("gpu"), GRHIAdapterName);
	Process->Properties.Add(TEXT("cpu"), FPlatformMisc::GetCPUBrand());
	Process->Properties.Add(TEXT("cpu-logical-cores"), FString::FromInt(FPlatformMisc::NumberOfCores()));
	Process->Properties.Add(TEXT("cpu-physical-cores"), FString::FromInt(FPlatformMisc::NumberOfCoresIncludingHyperthreads()));
	Process->Properties.Add(TEXT("ram_mb"), FString::FromInt(static_cast<int32>(FPlatformMemory::GetStats().TotalPhysical / (1024 * 1024))));

	TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink = MakeShared<HttpEventSink>(BaseUrl, Process, Auth, Sampling, Flusher);
	const size_t LOG_BUFFER_SIZE = 10 * 1024 * 1024;
	const size_t METRICS_BUFFER_SIZE = 10 * 1024 * 1024;
	const size_t THREAD_BUFFER_SIZE = 10 * 1024 * 1024;

	Dispatch::Init(&CreateGuid, Process, Sink, LOG_BUFFER_SIZE, METRICS_BUFFER_SIZE, THREAD_BUFFER_SIZE);
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Initializing Micromegas Telemetry process_id=%s"), *Process->ProcessId);
	return Sink;
}
