#pragma once
//
//  FormatTime.h
//
#include <string>

namespace LgnTracing
{
	struct DualTime;
}

std::string FormatTimeIso8601(const LgnTracing::DualTime& time);
