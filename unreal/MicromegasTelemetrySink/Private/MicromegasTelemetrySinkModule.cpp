#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "HAL/IConsoleManager.h"
#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"
#include "Templates/UniquePtr.h"

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;
	virtual void InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& auth) override;

private:
	void OnEnableTelemetryCommand();

	TUniquePtr<FAutoConsoleCommand> CmdEnableTelemetry;
	FString UploadBaseUrl;
	SharedTelemetryAuthenticator Authenticator;
};

//================================================================================
void FMicromegasTelemetrySinkModule::StartupModule()
{
	CmdEnableTelemetry.Reset(new FAutoConsoleCommand(TEXT("telemetry.enable"),
		TEXT("Initialized the telemetry system"),
		FConsoleCommandDelegate::CreateRaw(this, &FMicromegasTelemetrySinkModule::OnEnableTelemetryCommand)));
}

void FMicromegasTelemetrySinkModule::OnEnableTelemetryCommand()
{
	check(Authenticator.IsValid());
	TSharedPtr<MicromegasTracing::EventSink, ESPMode::ThreadSafe> Sink = InitHttpEventSink(UploadBaseUrl, Authenticator);
	Authenticator->Init(Sink);
	CmdEnableTelemetry.Reset();
}

void FMicromegasTelemetrySinkModule::ShutdownModule()
{
	CmdEnableTelemetry.Reset();
}

void FMicromegasTelemetrySinkModule::InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth)
{
	UploadBaseUrl = BaseUrl;
	Authenticator = Auth;
}

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)

ITelemetryAuthenticator::~ITelemetryAuthenticator()
{
}
