#pragma once
//
//  MicromegasTracing/ProcessInfo.h
//
#include <string>
#include "MicromegasTracing/DualTime.h"

namespace MicromegasTracing
{
	struct ProcessInfo
	{
		std::wstring ProcessId;
		std::wstring ParentProcessId;
		std::wstring Exe;
		std::wstring Username;
		std::wstring Computer;
		std::wstring Distro;
		std::wstring CpuBrand;
		uint64 TscFrequency;
		DualTime StartTime;
	};
} // namespace MicromegasTracing
