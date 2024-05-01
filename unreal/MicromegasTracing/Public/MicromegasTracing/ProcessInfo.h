#pragma once
//
//  MicromegasTracing/ProcessInfo.h
//
#include "MicromegasTracing/DualTime.h"
#include "Containers/Map.h"

namespace MicromegasTracing
{
	struct ProcessInfo
	{
		FString ProcessId;
		FString ParentProcessId;
		FString Exe;
		FString Username;
		FString Computer;
		FString Distro;
		FString CpuBrand;
		uint64 TscFrequency;
		DualTime StartTime;
		TMap<FString,FString> Properties;
	};
} // namespace MicromegasTracing
