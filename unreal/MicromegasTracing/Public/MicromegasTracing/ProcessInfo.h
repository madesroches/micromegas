#pragma once
//
//  MicromegasTracing/ProcessInfo.h
//
#include "MicromegasTracing/DualTime.h"

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
	};
} // namespace MicromegasTracing
