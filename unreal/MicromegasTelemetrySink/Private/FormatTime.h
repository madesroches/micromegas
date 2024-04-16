#pragma once
//
//  FormatTime.h
//
namespace MicromegasTracing
{
	struct DualTime;
}

FString FormatTimeIso8601(const MicromegasTracing::DualTime& time);
