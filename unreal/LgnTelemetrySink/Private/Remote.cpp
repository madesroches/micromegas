#include "LgnTelemetrySink/Remote.h"
#include "LgnTelemetrySink/Log.h"
#include "LgnTelemetrySink/FlushMonitor.h"
#include "LgnTracing/EventSink.h"
#include "LgnTracing/ProcessInfo.h"
#include "LgnTracing/LogBlock.h"
#include "LgnTracing/Macros.h"
#include "LgnTracing/EventStream.h"
#include "HttpModule.h"
#include "Interfaces/IHttpResponse.h"
#include "InsertStreamRequest.h"
#include "InsertProcessRequest.h"
#include "InsertBlockRequest.h"
#include "LogDependencies.h"
#include <string>
#include <sstream>
#include <functional>
#include "Serialization/JsonWriter.h"
#include "Serialization/JsonSerializer.h"
#include "HAL/Runnable.h"
#include "HAL/RunnableThread.h"
#include "HAL/ThreadManager.h"
#if PLATFORM_WINDOWS
#include "Windows/WindowsSystemIncludes.h"
#include "Windows/WindowsHWrapper.h"
#endif

DEFINE_LOG_CATEGORY(LogLgnTelemetrySink);

namespace
{
    void OnProcessRequestComplete(FHttpRequestPtr HttpRequest, FHttpResponsePtr HttpResponse, bool bSucceeded)
    {
        int32 code = HttpResponse ? HttpResponse->GetResponseCode() : 0;
        if ( !bSucceeded || code != 200 )
        {
            const TCHAR* status = TEXT("unknown");
            if ( HttpRequest )
            {
                status = ToString(HttpRequest->GetStatus());
            }
            UE_LOG(LogLgnTelemetrySink, Error, TEXT("Request completed with status=%s code=%d"), status, code);
        }
    }

    uint64 GetTscFrequency()
    {
#if PLATFORM_WINDOWS
        LARGE_INTEGER Frequency;
        verify( QueryPerformanceFrequency(&Frequency) );
        return Frequency.QuadPart;
#else
        return static_cast<uint64>( 1.0 / FPlatformTime::GetSecondsPerCycle64() );
#endif
    }
    
}


class RemoteSink : public LgnTracing::EventSink,
                   public FRunnable
{
public:
    explicit RemoteSink( const FString& baseUrl )
        : BaseUrl(baseUrl)
        , QueueSize( 0 )
        , RequestShutdown(false)
    {
        Thread.Reset(FRunnableThread::Create(this, TEXT("LgnRemoteTelemetrySink")));
        Flusher.Reset(new FlushMonitor(this));
    }
    
    virtual ~RemoteSink()
    {
    }

    //
    //  LgnTracing::EventSink
    //
    virtual void OnStartup( const LgnTracing::ProcessInfoPtr& processInfo )
    {
        FPlatformAtomics::InterlockedIncrement( &QueueSize );
        Queue.Enqueue( [this, processInfo]()
        {
            FString content = FormatInsertProcessRequest(*processInfo);
            SendJsonRequest( TEXT("process"), content );
        } );
        WakeupThread->Trigger();
    }
    
    virtual void OnShutdown()
    {
        LGN_LOG_STATIC(TEXT("LgnTelemetrySink"), LgnTracing::LogLevel::Info, TEXT("Shutting down") );
        Flusher.Reset();
        LgnTracing::FlushLogStream();
        LgnTracing::FlushMetricStream();
        RequestShutdown = true;
        WakeupThread->Trigger();
        Thread->WaitForCompletion();
    }
    
    virtual void OnInitLogStream( const LgnTracing::LogStreamPtr& stream )
    {
        IncrementQueueSize();
        Queue.Enqueue( [this, stream]()
        {
            FString content = FormatInsertLogStreamRequest( *stream );
            SendJsonRequest( TEXT("stream"), content );
        } );
        WakeupThread->Trigger();
    }

    virtual void OnInitMetricStream( const LgnTracing::MetricStreamPtr& stream )
    {
        IncrementQueueSize();
        Queue.Enqueue( [this, stream]()
        {
            FString content = FormatInsertMetricStreamRequest( *stream );
            SendJsonRequest( TEXT("stream"), content );
        } );
        WakeupThread->Trigger();
    }

    virtual void OnInitThreadStream( LgnTracing::ThreadStream* stream )
    {
        const uint32 threadId = FPlatformTLS::GetCurrentThreadId();
        const FString& threadName = FThreadManager::GetThreadName(threadId);

        stream->SetProperty( TEXT("thread-name"), *threadName );
        stream->SetProperty( TEXT("thread-id"), *FString::Format(TEXT("{0}"), {threadId}));

        IncrementQueueSize();
        Queue.Enqueue( [this, stream]()
        {
            FString content = FormatInsertThreadStreamRequest( *stream );
            SendJsonRequest( TEXT("stream"), content );
        } );
        WakeupThread->Trigger();
    }
    
    virtual void OnProcessLogBlock( const LgnTracing::LogBlockPtr& block )
    {
        IncrementQueueSize();
        Queue.Enqueue( [this, block]()
        {
            TArray<uint8> content = FormatBlockRequest(*block);
            SendBinaryRequest( TEXT("block"), content);
        } );
        WakeupThread->Trigger();
    }

    virtual void OnProcessMetricBlock( const LgnTracing::MetricsBlockPtr& block )
    {
        IncrementQueueSize();
        Queue.Enqueue( [this, block]()
        {
            TArray<uint8> content = FormatBlockRequest(*block);
            SendBinaryRequest( TEXT("block"), content);
        } );
        WakeupThread->Trigger();
    }

    virtual void OnProcessThreadBlock( const LgnTracing::ThreadBlockPtr& block )
    {
        LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("OnProcessThreadBlock"));
        IncrementQueueSize();
        Queue.Enqueue( [this, block]()
        {
            TArray<uint8> content = FormatBlockRequest(*block);
            SendBinaryRequest( TEXT("block"), content);
        } );
        WakeupThread->Trigger();
    }

    virtual bool IsBusy()
    {
        return QueueSize > 0;
    }

    //
    //  FRunnable
    //
    virtual uint32 Run()
    {
        while( true )
        {
            Callback c;
            while( Queue.Dequeue(c) )
            {
                int32 newQueueSize = FPlatformAtomics::InterlockedDecrement( &QueueSize );
                LGN_IMETRIC(TEXT("LgnTelemetrySink"), LgnTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), newQueueSize);
                c();
            }

            if ( RequestShutdown )
            {
                break;
            }
            WakeupThread->Wait();
        }
        return 0;
    }

private:

    void IncrementQueueSize()
    {
        LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("IncrementQueueSize"));
        int32 incrementedQueueSize = FPlatformAtomics::InterlockedIncrement( &QueueSize );
        LGN_IMETRIC(TEXT("LgnTelemetrySink"), LgnTracing::Verbosity::Min, TEXT("QueueSize"), TEXT("count"), incrementedQueueSize);
    }

    void SendJsonRequest( const TCHAR* command, const FString& content )
    {
        LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("SendJsonRequest"));
        TSharedRef<IHttpRequest, ESPMode::ThreadSafe> HttpRequest = FHttpModule::Get().CreateRequest();
        HttpRequest->SetURL(BaseUrl+command);
        HttpRequest->SetVerb(TEXT("PUT"));
        HttpRequest->SetContentAsString(content);
        HttpRequest->SetHeader( TEXT("Content-Type"), TEXT("application/json"));
        HttpRequest->OnProcessRequestComplete().BindStatic(&OnProcessRequestComplete);
        if ( !HttpRequest->ProcessRequest() )
        {
            UE_LOG(LogLgnTelemetrySink, Error, TEXT("Failed to initialize telemetry http request"));
        }
    }

    void SendBinaryRequest( const TCHAR* command, const TArray<uint8>& content )
    {
        LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("SendBinaryRequest"));
        TSharedRef<IHttpRequest, ESPMode::ThreadSafe> HttpRequest = FHttpModule::Get().CreateRequest();
        HttpRequest->SetURL(BaseUrl+command);
        HttpRequest->SetVerb(TEXT("PUT"));
        HttpRequest->SetContent(content);
        HttpRequest->SetHeader( TEXT("Content-Type"), TEXT("application/octet-stream"));
        HttpRequest->OnProcessRequestComplete().BindStatic(&OnProcessRequestComplete);
        if ( !HttpRequest->ProcessRequest() )
        {
            UE_LOG(LogLgnTelemetrySink, Error, TEXT("Failed to initialize telemetry http request"));
        }
    }
    
    typedef std::function<void()> Callback;
    typedef TQueue<Callback, EQueueMode::Mpsc> WorkQueue;
    FString BaseUrl;
    WorkQueue Queue;
    volatile int32 QueueSize;
    volatile bool RequestShutdown;
    FEventRef WakeupThread;
    TUniquePtr<FRunnableThread> Thread;
    TUniquePtr<FlushMonitor> Flusher;
};

std::wstring CreateGuid()
{
    return std::wstring( * FGuid::NewGuid().ToString(EGuidFormats::DigitsWithHyphens) );
}

std::wstring GetDistro()
{
    std::wostringstream str;
    str << ANSI_TO_TCHAR( FPlatformProperties::PlatformName() );
    str << TEXT(" ");
    str << *FPlatformMisc::GetOSVersion();
    return str.str();
}

void InitRemoteSink(){
    using namespace LgnTracing;
    UE_LOG(LogLgnTelemetrySink, Log, TEXT("Initializing Remote Telemetry Sink"));

    //const char* url = "https://web-api.live.playground.legionlabs.com/v1/spaces/default/telemetry/ingestion/";
    const char* url = "http://localhost:8081/v1/spaces/default/telemetry/ingestion/";
    std::shared_ptr<EventSink> sink = std::make_shared<RemoteSink>( url );
    const size_t LOG_BUFFER_SIZE = 10*1024*1024;
    const size_t METRICS_BUFFER_SIZE = 10*1024*1024;
    const size_t THREAD_BUFFER_SIZE = 10*1024*1024;

    std::wstring processId = CreateGuid();
    std::wstring parentProcessId = *FPlatformMisc::GetEnvironmentVariable(TEXT("LGN_TELEMETRY_PARENT_PROCESS"));
    FPlatformMisc::SetEnvironmentVar(TEXT("LGN_TELEMETRY_PARENT_PROCESS"), processId.c_str());

    DualTime startTime = DualTime::Now();

    ProcessInfoPtr process( new ProcessInfo() );
    process->ProcessId = processId;
    process->ParentProcessId = parentProcessId;
    process->Exe = FPlatformProcess::ExecutablePath();
    process->Username = FPlatformProcess::UserName(false);
    process->Computer = FPlatformProcess::ComputerName();
    process->Distro = GetDistro();
    process->CpuBrand = *FPlatformMisc::GetCPUBrand();
    process->TscFrequency = GetTscFrequency();
    process->StartTime = startTime;

    Dispatch::Init(&CreateGuid, process, sink, LOG_BUFFER_SIZE, METRICS_BUFFER_SIZE, THREAD_BUFFER_SIZE);
    UE_LOG(LogLgnTelemetrySink, Log, TEXT("Initializing Legion Telemetry for process %s"), process->ProcessId.c_str());
    LGN_LOG_STATIC(TEXT("LgnTelemetrySink"), LogLevel::Info, TEXT("Telemetry enabled") );
}
