#pragma once
//
//  MicromegasTracing/DualTime.h
//
#include "HAL/PlatformTime.h"

namespace MicromegasTracing
{
	struct DualTime
	{
		uint64 Timestamp;
		FDateTime SystemTime;

		DualTime()
			: Timestamp(0)
		{
		}

		DualTime(uint64 timestamp, const FDateTime& systemTime)
			: Timestamp(timestamp)
			, SystemTime(systemTime)

		{
		}

		static DualTime Now()
		{
			return DualTime(FPlatformTime::Cycles64(), FDateTime::UtcNow());
		}
	};
} // namespace MicromegasTracing
