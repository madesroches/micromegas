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
	static const FName ModuleName;

	FORCEINLINE static IMicromegasTelemetrySinkModule& LoadModuleChecked()
	{
		return FModuleManager::LoadModuleChecked<IMicromegasTelemetrySinkModule>(ModuleName);
	}

	FORCEINLINE static IMicromegasTelemetrySinkModule* GetModulePtr()
	{
		return FModuleManager::GetModulePtr<IMicromegasTelemetrySinkModule>(ModuleName);
	}

	virtual void InitTelemetry( const SharedTelemetryAuthenticator& auth ) = 0;
};
