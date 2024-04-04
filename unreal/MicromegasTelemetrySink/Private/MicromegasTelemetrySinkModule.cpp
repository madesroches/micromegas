#include "MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h"

//================================================================================
class FMicromegasTelemetrySinkModule : public IMicromegasTelemetrySinkModule
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;
};

//================================================================================
void FMicromegasTelemetrySinkModule::StartupModule()
{
}

void FMicromegasTelemetrySinkModule::ShutdownModule()
{
}

const FName IMicromegasTelemetrySinkModule::ModuleName("MicromegasTelemetrySink");

IMPLEMENT_MODULE(FMicromegasTelemetrySinkModule, MicromegasTelemetrySink)
