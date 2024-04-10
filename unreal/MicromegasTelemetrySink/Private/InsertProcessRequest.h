#pragma once
//
//  MicromegasTelemetrySink/InsertProcessRequest.h
//

namespace MicromegasTracing
{
	struct ProcessInfo;
}

TArray<uint8> FormatInsertProcessRequest(const MicromegasTracing::ProcessInfo& processInfo);
