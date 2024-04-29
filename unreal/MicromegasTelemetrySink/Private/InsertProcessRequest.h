#pragma once
//
//  MicromegasTelemetrySink/InsertProcessRequest.h
//
#include "HAL/Platform.h"
#include "Containers/Array.h"

namespace MicromegasTracing
{
	struct ProcessInfo;
}

TArray<uint8> FormatInsertProcessRequest(const MicromegasTracing::ProcessInfo& processInfo);
