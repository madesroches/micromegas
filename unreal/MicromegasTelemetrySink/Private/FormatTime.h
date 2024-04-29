#pragma once
//
//  FormatTime.h
//
#include "Containers/UnrealString.h"
namespace MicromegasTracing
{
	struct DualTime;
}

FString FormatTimeIso8601(const MicromegasTracing::DualTime& time);
