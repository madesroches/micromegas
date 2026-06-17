#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "BuildSettings.h"
#include "HAL/PlatformProcess.h"
#include "HAL/Runnable.h"
#include "HAL/RunnableThread.h"
#include "HAL/ThreadManager.h"
#include "HttpModule.h"
#include "HttpRetrySystem.h"
#include "Modules/ModuleManager.h"
#include "InsertBlockRequest.h"
#include "InsertProcessRequest.h"
#include "InsertStreamRequest.h"
#include "Interfaces/IHttpResponse.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/EventStream.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "Misc/App.h"
#include "Misc/EngineVersion.h"
#include "Policies/CondensedJsonPrintPolicy.h"
#include "Serialization/JsonWriter.h"

#if PLATFORM_WINDOWS
	#include "Windows/WindowsSystemIncludes.h"
	#include "Windows/WindowsHWrapper.h"
#endif

DEFINE_LOG_CATEGORY(LogMicromegasTelemetrySink);

namespace
{
	// Retry counts and total retry-window budgets (seconds) indexed by EUploadPriority.
	// The window is also the per-attempt socket timeout — see feature doc for rationale.
	constexpr uint32 RetryCountByPriority[]         = { 10, 5, 2, 1 };
	constexpr double RetryWindowSecondsByPriority[]  = { 300.0, 120.0, 30.0, 6.0 };

	// Soft cap: above this Traces are dropped first.
	TAutoConsoleVariable<int32> CVarMaxQueueBytes(
		TEXT("telemetry.max_queue_bytes"),
		128 * 1024 * 1024,
		TEXT("Soft queue byte cap. Traces dropped above this threshold."));

	// Hard ceiling: above this Logs/Metrics are dropped too. Bounds memory for any outage.
	TAutoConsoleVariable<int32> CVarHardQueueBytes(
		TEXT("telemetry.hard_queue_bytes"),
		256 * 1024 * 1024,
		TEXT("Hard queue byte ceiling. Logs/Metrics dropped above this threshold. Must be >= telemetry.max_queue_bytes."));

	// Max uploads handed to FHttpRetrySystem at once. The worker stops draining once this many are
	// outstanding, keeping the outage backlog in our priority queues where the byte cap governs it.
	TAutoConsoleVariable<int32> CVarMaxInFlightRequests(
		TEXT("telemetry.max_in_flight_requests"),
		3,
		TEXT("Max concurrent telemetry uploads in flight. Worker pauses draining above this."));

	constexpr float ShutdownFlushTimeoutSeconds = 5.0f;

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

	FString GetCommandLineArgumentsAsJsonArray(const TCHAR* const CommandLine)
	{
		FString ArgAsJsonArray;
		TSharedRef<TJsonWriter<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>> JsonWriter = TJsonWriterFactory<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>::Create(&ArgAsJsonArray);

		JsonWriter->WriteArrayStart();
		{
			FString NextArg;
			const TCHAR* ParsedCommandLine = CommandLine;
			while (FParse::Token(ParsedCommandLine, NextArg, false))
			{
				JsonWriter->WriteValue(NextArg);
			}
		}
		JsonWriter->WriteArrayEnd();
		JsonWriter->Close();

		return ArgAsJsonArray;
	}

	void OnHttpRequestComplete(
		FHttpRequestPtr Req,
		FHttpResponsePtr Resp,
		bool bSucceeded,
		uint64 StartTimestamp,
		TSharedPtr<FCompletionState, ESPMode::ThreadSafe> State)
	{
		// Fires once at terminal resolution (success or retries exhausted).
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpRequestCompletionTime"), TEXT("ticks"), FPlatformTime::Cycles64() - StartTimestamp);
		--State->HttpInFlightRequests;
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpInFlightRequests"), TEXT("count"), static_cast<uint64>(State->HttpInFlightRequests.load()));
		State->WakeupThread->Trigger(); // freed a gate slot — wake the worker to drain the next queued item

		const int32 Code = Resp ? Resp->GetResponseCode() : 0;
		if (Resp)
		{
			MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpResponseCode"), TEXT("code"), static_cast<uint64>(Code));
		}

		if (!bSucceeded || Code != 200)
		{
			FString Reason;
			if (Resp)
			{
				Reason = LexToString(Resp->GetFailureReason());
			}
			else if (Req)
			{
				Reason = LexToString(Req->GetFailureReason());
			}
			UE_LOG(LogMicromegasTelemetrySink, Warning,
				TEXT("Request completed with code=%d reason=%s response=%s"),
				Code,
				*Reason,
				Resp ? *(Resp->GetContentAsString()) : TEXT(""));
		}
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
	, Flusher(InFlusher)
{
	State = MakeShared<FCompletionState, ESPMode::ThreadSafe>();
	RetryManager = MakeShared<FHttpRetrySystem::FManager>(
		FHttpRetrySystem::FRetryLimitCountSetting(),
		FHttpRetrySystem::FRetryTimeoutRelativeSecondsSetting());
	Thread.Reset(FRunnableThread::Create(this, TEXT("MicromegasHttpTelemetrySink")));
}

HttpEventSink::~HttpEventSink()
{
}

void HttpEventSink::OnAuthUpdated()
{
	State->WakeupThread->Trigger();
}

void HttpEventSink::OnStartup(const MicromegasTracing::ProcessInfoPtr& ProcessInfo)
{
	MicromegasTracing::Dispatch::InitCurrentThreadStream();
	EnqueueWithPriority(EUploadPriority::Metadata, 0, [this, ProcessInfo]()
		{
			const TArray<uint8> Body = FormatInsertProcessRequest(*ProcessInfo);
			SendBinaryRequest(TEXT("insert_process"), Body, EUploadPriority::Metadata);
		});
}

void HttpEventSink::OnShutdown()
{
	RequestShutdown = true;
	State->WakeupThread->Trigger();
	Thread->WaitForCompletion();
	if (RetryManager.IsValid() && FModuleManager::Get().IsModuleLoaded("HTTP"))
	{
		RetryManager->BlockUntilFlushed(ShutdownFlushTimeoutSeconds);
	}
}

void HttpEventSink::OnInitLogStream(const MicromegasTracing::LogStreamPtr& Stream)
{
	EnqueueStreamInit(EUploadPriority::Metadata, [this, Stream]() -> TArray<uint8>
		{
			return FormatInsertLogStreamRequest(*Stream);
		});
}

void HttpEventSink::OnInitMetricStream(const MicromegasTracing::MetricStreamPtr& Stream)
{
	EnqueueStreamInit(EUploadPriority::Metadata, [this, Stream]() -> TArray<uint8>
		{
			return FormatInsertMetricStreamRequest(*Stream);
		});
}

void HttpEventSink::OnInitNetStream(const MicromegasTracing::NetStreamPtr& Stream)
{
	EnqueueStreamInit(EUploadPriority::Metadata, [this, Stream]() -> TArray<uint8>
		{
			return FormatInsertNetStreamRequest(*Stream);
		});
}

void HttpEventSink::OnInitThreadStream(MicromegasTracing::ThreadStream* Stream)
{
	const uint32 ThreadId = FPlatformTLS::GetCurrentThreadId();
	const FString* ThreadName = &FThreadManager::GetThreadName(ThreadId);
	if (ThreadName->IsEmpty())
	{
		static const FString UnnamedThread{ TEXT("UnnamedThread") };
		ThreadName = &UnnamedThread;
	}

	Stream->SetProperty(TEXT("thread-name"), *ThreadName);
	Stream->SetProperty(TEXT("thread-id"), FString::Format(TEXT("{0}"), { ThreadId }));

	EnqueueWithPriority(EUploadPriority::Metadata, 0, [this, Stream]()
		{
			const TArray<uint8> Body = FormatInsertThreadStreamRequest(*Stream);
			SendBinaryRequest(TEXT("insert_stream"), Body, EUploadPriority::Metadata);
		});
}

void HttpEventSink::OnProcessLogBlock(const MicromegasTracing::LogBlockPtr& Block)
{
	EnqueueBlock(Block, EUploadPriority::Logs);
}

void HttpEventSink::OnProcessMetricBlock(const MicromegasTracing::MetricsBlockPtr& Block)
{
	EnqueueBlock(Block, EUploadPriority::Metrics);
}

void HttpEventSink::OnProcessThreadBlock(const MicromegasTracing::ThreadBlockPtr& Block)
{
	EnqueueBlock(Block, EUploadPriority::Traces);
}

void HttpEventSink::OnProcessNetBlock(const MicromegasTracing::NetBlockPtr& Block)
{
	EnqueueBlock(Block, EUploadPriority::Traces);
}

bool HttpEventSink::IsBusy()
{
	return QueueSize > 0;
}

bool HttpEventSink::DrainOneItem()
{
	if (State->HttpInFlightRequests.load() >= static_cast<int64>(CVarMaxInFlightRequests.GetValueOnAnyThread()))
	{
		return false; // gate closed — leave items queued, worker will sleep until a slot frees
	}

	WorkQueue* Queues[] = { &MetadataQueue, &LogQueue, &MetricQueue, &TraceQueue };
	for (WorkQueue* Q : Queues)
	{
		FQueuedWork Item;
		if (Q->Dequeue(Item))
		{
			DecrementQueueSize();
			QueueSizeBytes -= Item.PayloadBytes;
			MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSizeBytes"), TEXT("bytes"), QueueSizeBytes.load());
			Item.Work();
			return true;
		}
	}
	return false;
}

uint32 HttpEventSink::Run()
{
	while (true)
	{
		if (Auth->IsReady())
		{
			while (DrainOneItem())
			{
			}
		}
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpInFlightRequests"), TEXT("count"), static_cast<uint64>(State->HttpInFlightRequests.load()));

		if (RequestShutdown)
		{
			// Final drain: bypass auth check, submit any remaining queued work before exit.
			WorkQueue* Queues[] = { &MetadataQueue, &LogQueue, &MetricQueue, &TraceQueue };
			for (WorkQueue* Q : Queues)
			{
				FQueuedWork Item;
				while (Q->Dequeue(Item))
				{
					DecrementQueueSize();
					QueueSizeBytes -= Item.PayloadBytes;
					Item.Work();
				}
			}
			return 0;
		}

		const double WaitSeconds = Flusher->Tick(this);
		State->WakeupThread->Wait(static_cast<uint32>(FMath::Max(0.0, WaitSeconds) * 1000.0));
	}
}

void HttpEventSink::IncrementQueueSize()
{
	int32 IncrementedQueueSize = ++QueueSize;
	MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), IncrementedQueueSize);
}

void HttpEventSink::DecrementQueueSize()
{
	int32 NewQueueSize = --QueueSize;
	MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), NewQueueSize);
}

auto HttpEventSink::QueueForPriority(EUploadPriority UploadPriority) -> WorkQueue&
{
	switch (UploadPriority)
	{
	case EUploadPriority::Metadata: return MetadataQueue;
	case EUploadPriority::Logs:     return LogQueue;
	case EUploadPriority::Metrics:  return MetricQueue;
	default:                        return TraceQueue;
	}
}

void HttpEventSink::EnqueueWithPriority(EUploadPriority Priority, int32 PayloadBytes, Callback Work)
{
	const int64 SoftCap = static_cast<int64>(CVarMaxQueueBytes.GetValueOnAnyThread());
	const int64 HardCap = FMath::Max(static_cast<int64>(CVarHardQueueBytes.GetValueOnAnyThread()), SoftCap);

	if (Priority == EUploadPriority::Traces && QueueSizeBytes.load() >= SoftCap)
	{
		int64 Dropped = ++DroppedUploads;
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("DroppedUploads"), TEXT("count"), static_cast<uint64>(Dropped));
		return;
	}

	if (Priority != EUploadPriority::Metadata && QueueSizeBytes.load() >= HardCap)
	{
		int64 Dropped = ++DroppedUploads;
		MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("DroppedUploads"), TEXT("count"), static_cast<uint64>(Dropped));
		return;
	}

	// Charge count and bytes before Enqueue: the worker may dequeue and de-charge
	// immediately, driving both counters transiently negative otherwise.
	IncrementQueueSize();
	QueueSizeBytes += PayloadBytes;
	MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("QueueSizeBytes"), TEXT("bytes"), QueueSizeBytes.load());
	QueueForPriority(Priority).Enqueue(FQueuedWork(MoveTemp(Work), PayloadBytes));
	State->WakeupThread->Trigger();
}

void HttpEventSink::EnqueueStreamInit(EUploadPriority Priority, TFunction<TArray<uint8>()> FormatFn)
{
	EnqueueWithPriority(Priority, 0, [this, FormatFn = MoveTemp(FormatFn), Priority]()
		{
			const TArray<uint8> Body = FormatFn();
			SendBinaryRequest(TEXT("insert_stream"), Body, Priority);
		});
}

template <typename BlockPtrT>
void HttpEventSink::EnqueueBlock(const BlockPtrT& Block, EUploadPriority Priority)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (!Sampling->ShouldSampleBlock(Block))
	{
		return;
	}
	const int32 PayloadBytes = static_cast<int32>(Block->GetEvents().GetSizeBytes());
	EnqueueWithPriority(Priority, PayloadBytes, [this, Block, Priority]()
		{
			const TArray<uint8> Content = FormatBlockRequest(*Process, *Block);
			SendBinaryRequest(TEXT("insert_block"), Content, Priority);
		});
}

void HttpEventSink::SendBinaryRequest(const TCHAR* Command, const TArray<uint8>& Content, EUploadPriority Priority)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");

	if (!FModuleManager::Get().IsModuleLoaded("HTTP"))
	{
		return;
	}

	const uint8 PriorityIndex = static_cast<uint8>(Priority);
	const uint32 RetryLimit   = RetryCountByPriority[PriorityIndex];
	const double RetryWindow  = RetryWindowSecondsByPriority[PriorityIndex];

	// 429=rate-limited, 500=internal, 502/503=gateway/unavailable, 504=gateway-timeout
	static const FHttpRetrySystem::FRetryResponseCodes RetryCodes({ 429, 500, 502, 503, 504 });
	static const FHttpRetrySystem::FRetryVerbs RetryVerbs({ FName(TEXT("POST")) });
	FHttpRetrySystem::FExponentialBackoffCurve Backoff;

	TSharedRef<FHttpRetrySystem::FRequest, ESPMode::ThreadSafe> HttpRequest =
		RetryManager->CreateRequest(
			FHttpRetrySystem::FRetryLimitCountSetting(RetryLimit),
			FHttpRetrySystem::FRetryTimeoutRelativeSecondsSetting(RetryWindow),
			RetryCodes,
			RetryVerbs,
			FHttpRetrySystem::FRetryDomainsPtr(),
			FHttpRetrySystem::FRetryLimitCountSetting(),
			Backoff);

	HttpRequest->SetURL(BaseUrl + Command);
	HttpRequest->SetVerb(TEXT("POST"));
	HttpRequest->SetContent(Content);
	HttpRequest->SetHeader(TEXT("Content-Type"), TEXT("application/octet-stream"));
	HttpRequest->SetDelegateThreadPolicy(EHttpRequestDelegateThreadPolicy::CompleteOnHttpThread);

	const uint64 StartTimestamp = FPlatformTime::Cycles64();
	TSharedPtr<FCompletionState, ESPMode::ThreadSafe> StateCopy = State;

	HttpRequest->OnProcessRequestComplete().BindLambda(
		[StartTimestamp, StateCopy](FHttpRequestPtr Req, FHttpResponsePtr Resp, bool bSucceeded)
		{
			OnHttpRequestComplete(Req, Resp, bSucceeded, StartTimestamp, StateCopy);
		});

	HttpRequest->OnRequestWillRetry().BindLambda(
		[](FHttpRequestPtr Req, FHttpResponsePtr Resp, float SecondsToRetry)
		{
			const int32 Code = Resp ? Resp->GetResponseCode() : 0;
			MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpRetryResponseCode"), TEXT("code"), static_cast<uint64>(Code));
			MICROMEGAS_FMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpRetryAttemptElapsed"), TEXT("seconds"), Req->GetElapsedTime());
		});

	if (!Auth->Sign(*HttpRequest))
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to sign telemetry http request"));
		return;
	}

	// Charge the gate before ProcessRequest: completion fires on the HTTP thread and
	// could otherwise decrement before this increment, driving the counter negative.
	++State->HttpInFlightRequests;
	MICROMEGAS_IMETRIC("MicromegasTelemetrySink", MicromegasTracing::Verbosity::Min, TEXT("HttpInFlightRequests"), TEXT("count"), static_cast<uint64>(State->HttpInFlightRequests.load()));
	if (!HttpRequest->ProcessRequest())
	{
		--State->HttpInFlightRequests; // no completion delegate will fire
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("Failed to initialize telemetry http request"));
	}
}

FString CreateGuid()
{
	return FGuid::NewGuid().ToString(EGuidFormats::DigitsWithHyphensLower);
}

FString GetDistro()
{
	return FString::Printf(TEXT("%s %s"), ANSI_TO_TCHAR(FPlatformProperties::PlatformName()), *FPlatformMisc::GetOSVersion());
}

TSharedPtr<MicromegasTracing::EventSink> InitHttpEventSink(
	const FString& BaseUrl,
	const SharedTelemetryAuthenticator& Auth,
	const SharedSamplingController& Sampling,
	const SharedFlushMonitor& Flusher,
	const TMap<FString,FString>& AdditionalProcessProperties)
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
	// use of underscore is to match other micromegas aware application.
	Process->Properties.Add(TEXT("exe_args"), GetCommandLineArgumentsAsJsonArray(FCommandLine::GetOriginalForLogging()));
	Process->Properties.Add(TEXT("command-line"), FCommandLine::GetOriginalForLogging());

	// Would be 0 on local builds
	Process->Properties.Add(TEXT("commit"), FString::FromInt(BuildSettings::GetCurrentChangelist()));

	// note that the following doesn't necessarily get the device in use by RHI Adapter
    // but not to have to wait for the graphics init or depend on the graphics
    // it makes up for a good candidate still, especially on the prod floor
    // that is single adapter dominated
	Process->Properties.Add(TEXT("gpu"), FPlatformMisc::GetPrimaryGPUBrand());
	Process->Properties.Add(TEXT("cpu"), FPlatformMisc::GetCPUBrand());
	Process->Properties.Add(TEXT("cpu-physical-cores"), FString::FromInt(FPlatformMisc::NumberOfCores()));
	Process->Properties.Add(TEXT("cpu-logical-cores"), FString::FromInt(FPlatformMisc::NumberOfCoresIncludingHyperthreads()));
    // this is not a typo, _ was chosen to delimit the unit
	Process->Properties.Add(TEXT("ram_mb"), FString::FromInt(static_cast<int32>(FPlatformMemory::GetStats().TotalPhysical / (1024 * 1024))));

	for (const TPair<FString,FString>& Pair : AdditionalProcessProperties)
	{
		Process->Properties.Add(Pair.Key, Pair.Value);
	}

	TSharedPtr<EventSink> Sink = MakeShared<HttpEventSink>(BaseUrl, Process, Auth, Sampling, Flusher);
	constexpr size_t LogBufferSize = 10 * 1024 * 1024;
	constexpr size_t MetricsBufferSize = 10 * 1024 * 1024;
	constexpr size_t ThreadBufferSize = 10 * 1024 * 1024;
	constexpr size_t NetBufferSize = 8 * 1024 * 1024;

	Dispatch::Init(&CreateGuid, Process, Sink, LogBufferSize, MetricsBufferSize, ThreadBufferSize, NetBufferSize, Sampling->GetNetVerbosity());
	UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Initializing Micromegas Telemetry process_id=%s"), *Process->ProcessId);
	return Sink;
}
