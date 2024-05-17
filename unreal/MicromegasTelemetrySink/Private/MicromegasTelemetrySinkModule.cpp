#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "HAL/IConsoleManager.h"
#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "MicromegasTelemetrySink/LogInterop.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "SamplingController.h"
#include "Templates/UniquePtr.h"

#define MICROMEGAS_ENABLE_TELEMETRY_ON_START 0

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;
	virtual void InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& auth) override;

private:
	void OnEnableTelemetry();
	void RegisterConsoleVariables();

	TUniquePtr<FAutoConsoleCommand> CmdEnableTelemetry;
	FString UploadBaseUrl;
	SharedTelemetryAuthenticator Authenticator;
	SharedSampingController SamplingController;
};

//================================================================================
void FMicromegasTelemetrySinkModule::StartupModule()
{
#if MICROMEGAS_ENABLE_TELEMETRY_ON_START == 0
	CmdEnableTelemetry.Reset(new FAutoConsoleCommand(TEXT("telemetry.enable"),
		TEXT("Initialized the telemetry system"),
		FConsoleCommandDelegate::CreateRaw(this, &FMicromegasTelemetrySinkModule::OnEnableTelemetry)));
#endif // MICROMEGAS_ENABLE_TELEMETRY_ON_START
}

void FMicromegasTelemetrySinkModule::OnEnableTelemetry()
{
	check(Authenticator.IsValid());
	SamplingController = MakeShared<FSamplingController>();
	TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink = InitHttpEventSink(UploadBaseUrl, Authenticator, SamplingController);
	Authenticator->Init(Sink);
	CmdEnableTelemetry.Reset();
	InitLogInterop();
}

void FMicromegasTelemetrySinkModule::ShutdownModule()
{
	CmdEnableTelemetry.Reset();
}

void FMicromegasTelemetrySinkModule::InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth)
{
	UploadBaseUrl = BaseUrl;
	Authenticator = Auth;

#if MICROMEGAS_ENABLE_TELEMETRY_ON_START
	OnEnableTelemetry();
#endif // MICROMEGAS_ENABLE_TELEMETRY_ON_START
}

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)

ITelemetryAuthenticator::~ITelemetryAuthenticator()
{
}
