#pragma once

#include "Modules/ModuleInterface.h"
#include "Modules/ModuleManager.h"

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
};
