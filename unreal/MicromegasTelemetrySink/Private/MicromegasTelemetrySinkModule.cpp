#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"
#include "MicromegasTelemetrySink/HttpEventSink.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;
	virtual void InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& auth) override;
};

//================================================================================
void FMicromegasTelemetrySinkModule::StartupModule()
{
}

void FMicromegasTelemetrySinkModule::ShutdownModule()
{
}

void FMicromegasTelemetrySinkModule::InitTelemetry(const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth)
{
	InitRemoteSink(BaseUrl, Auth);
}

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)

ITelemetryAuthenticator::~ITelemetryAuthenticator()
{
}
