#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "HAL/IConsoleManager.h"
#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "MicromegasTelemetrySink/LogInterop.h"
#include "MicromegasTelemetrySink/MetricPublisher.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "MicromegasTracing/Dispatch.h"
#include "Misc/CoreDelegates.h"
#include "SamplingController.h"
#include "Templates/UniquePtr.h"

#define MICROMEGAS_ENABLE_TELEMETRY_ON_START 1

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void PreUnloadCallback() override;
	virtual void ShutdownModule() override;
	virtual void InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& auth) override;

private:
	void OnEnable();
	void OnFlush();
	void RegisterConsoleVariables();

	TUniquePtr<FAutoConsoleCommand> CmdEnable;
	TUniquePtr<FAutoConsoleCommand> CmdFlush;
	FString UploadBaseUrl;
	SharedTelemetryAuthenticator Authenticator;
	SharedSamplingController SamplingController;
	SharedFlushMonitor Flusher;
	SharedMetricPublisher MetricPub;
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
	MetricPub = MakeShared<MetricPublisher>();
	TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink = InitHttpEventSink(UploadBaseUrl, Authenticator, SamplingController, Flusher);
	Authenticator->Init(Sink);
	CmdEnable.Reset();
	InitLogInterop();
	CmdFlush.Reset(new FAutoConsoleCommand(TEXT("telemetry.flush"),
		TEXT("Marks telemetry buffers as full"),
		FConsoleCommandDelegate::CreateRaw(this, &FMicromegasTelemetrySinkModule::OnFlush)));

	FCoreDelegates::OnCommandletPostMain.AddRaw(this, &FMicromegasTelemetrySinkModule::OnFlush);
}

void FMicromegasTelemetrySinkModule::OnFlush()
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

void FMicromegasTelemetrySinkModule::InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth)
{
	UploadBaseUrl = BaseUrl;
	Authenticator = Auth;

#if MICROMEGAS_ENABLE_TELEMETRY_ON_START
	OnEnable();
#endif // MICROMEGAS_ENABLE_TELEMETRY_ON_START
}

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)

ITelemetryAuthenticator::~ITelemetryAuthenticator()
{
}
