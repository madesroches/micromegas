//
//  FormatTime.cpp
//
#include "FormatTime.h"
#include "MicromegasTracing/DualTime.h"

FString FormatTimeIso8601(const MicromegasTracing::DualTime& dualTime)
{
	return dualTime.SystemTime.ToIso8601();
}
