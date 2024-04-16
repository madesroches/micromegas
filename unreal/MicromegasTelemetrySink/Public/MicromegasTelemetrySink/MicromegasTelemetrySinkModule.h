#pragma once

//
//  MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h
//

#include "Modules/ModuleInterface.h"
#include "Modules/ModuleManager.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"


class MICROMEGASTELEMETRYSINK_API IMicromegasTelemetrySinkModule : public IModuleInterface
{
public:
	static FName GetModuleName()
	{
		// will be ok even if the dll is not loaded yet
		// should not be used frequently
		return FName("MicromegasTelemetrySink");
	}

	static IMicromegasTelemetrySinkModule& LoadModuleChecked()
	{
		return FModuleManager::LoadModuleChecked<IMicromegasTelemetrySinkModule>(GetModuleName());
	}

	static IMicromegasTelemetrySinkModule* GetModulePtr()
	{
		return FModuleManager::GetModulePtr<IMicromegasTelemetrySinkModule>(GetModuleName());
	}

	virtual void InitTelemetry( const FString& BaseUrl, const SharedTelemetryAuthenticator& Auth ) = 0;
};
