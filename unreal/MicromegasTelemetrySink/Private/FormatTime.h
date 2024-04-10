#pragma once
//
//  FormatTime.h
//
#include <string>

namespace MicromegasTracing
{
	struct DualTime;
}

std::string FormatTimeIso8601(const MicromegasTracing::DualTime& time);
