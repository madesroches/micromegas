#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"

#include "Containers/Map.h"
#include "HAL/IConsoleManager.h"
#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "MicromegasTelemetrySink/LogInterop.h"
#include "MicromegasTelemetrySink/MetricPublisher.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "MicromegasTracing/Dispatch.h"
#include "Misc/CoreDelegates.h"
#include "SamplingController.h"
#include "SystemErrorReporter.h"
#include "Templates/UniquePtr.h"

#define MICROMEGAS_ENABLE_TELEMETRY_ON_START 1

#if PLATFORM_WINDOWS
	#define MICROMEGAS_CRASH_REPORTING 1
#else
	#define MICROMEGAS_CRASH_REPORTING 0
#endif

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void PreUnloadCallback() override;
	virtual void ShutdownModule() override;
	virtual void InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth, const TMap<FString, FString>& InAdditionalProcessProperties /*= TMap<FString, FString>()*/) override;

private:
	void OnEnable();
	void OnFlush() const;

	TUniquePtr<FAutoConsoleCommand> CmdEnable;
	TUniquePtr<FAutoConsoleCommand> CmdFlush;
	FString UploadBaseUrl;
	SharedTelemetryAuthenticator Authenticator;
	SharedSamplingController SamplingController;
	SharedFlushMonitor Flusher;
	SharedMetricPublisher MetricPub;
	TUniquePtr<FSystemErrorReporter> SystemErrorReporter;
	TMap<FString, FString> AdditionalProcessProperties;
};

//================================================================================
void FMicromegasTelemetrySinkModule::StartupModule()
{
#if MICROMEGAS_ENABLE_TELEMETRY_ON_START == 0
	CmdEnable.Reset(new FAutoConsoleCommand(TEXT("telemetry.enable"),
		TEXT("Initializes the telemetry system"),
		FConsoleCommandDelegate::CreateRaw(this, &FMicromegasTelemetrySinkModule::OnEnable)));
#endif // MICROMEGAS_ENABLE_TELEMETRY_ON_START
}

void FMicromegasTelemetrySinkModule::OnEnable()
{
	const FName HttpModuleName = TEXT("HTTP");
	FModuleManager::Get().LoadModuleChecked(HttpModuleName);
	check(Authenticator.IsValid());
	Flusher = MakeShared<FlushMonitor>();
	SamplingController = MakeShared<FSamplingController>(Flusher);

	const TSharedPtr<MicromegasTracing::EventSink> Sink = InitHttpEventSink(UploadBaseUrl, Authenticator, SamplingController, Flusher, AdditionalProcessProperties);
	Authenticator->Init(Sink);
	MetricPub = MakeShared<MetricPublisher>();
	CmdEnable.Reset();
	InitLogInterop();
#if MICROMEGAS_CRASH_REPORTING
	SystemErrorReporter.Reset(new FSystemErrorReporter());
#endif
	CmdFlush.Reset(new FAutoConsoleCommand(TEXT("telemetry.flush"),
		TEXT("Marks telemetry buffers as full"),
		FConsoleCommandDelegate::CreateRaw(this, &FMicromegasTelemetrySinkModule::OnFlush)));

	FCoreDelegates::OnCommandletPostMain.AddRaw(this, &FMicromegasTelemetrySinkModule::OnFlush);
}

void FMicromegasTelemetrySinkModule::OnFlush() const
{
	if (Flusher.IsValid())
	{
		Flusher->Flush();
	}
}

void FMicromegasTelemetrySinkModule::PreUnloadCallback()
{
	OnFlush();

	CmdEnable.Reset();
	CmdFlush.Reset();
	MetricPub.Reset();
	Authenticator.Reset();
	SamplingController.Reset();
	Flusher.Reset();
}

void FMicromegasTelemetrySinkModule::ShutdownModule()
{
	FCoreDelegates::OnCommandletPostMain.RemoveAll(this);
	MicromegasTracing::Dispatch::Shutdown();
}

void FMicromegasTelemetrySinkModule::InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth, const TMap<FString, FString>& InAdditionalProcessProperties /*= TMap<FString, FString>()*/)
{
	UploadBaseUrl = BaseUrl;
	Authenticator = Auth;
	this->AdditionalProcessProperties = InAdditionalProcessProperties;

#if MICROMEGAS_ENABLE_TELEMETRY_ON_START
	OnEnable();
#endif // MICROMEGAS_ENABLE_TELEMETRY_ON_START
}

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)

ITelemetryAuthenticator::~ITelemetryAuthenticator()
{
}
