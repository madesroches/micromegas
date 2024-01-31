#pragma once
//
//  LgnTracing/ProcessInfo.h
//
#include <string>
#include "LgnTracing/DualTime.h"

namespace LgnTracing
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
        uint64  TscFrequency;
        DualTime StartTime;
    };
}
