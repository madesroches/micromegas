#pragma once
//
//  MicromegasTelemetrySink/InsertStreamRequest.h
//
#include "MicromegasTracing/Fwd.h"

TArray<uint8> FormatInsertLogStreamRequest(const MicromegasTracing::LogStream& stream);
TArray<uint8> FormatInsertMetricStreamRequest(const MicromegasTracing::MetricStream& stream);
TArray<uint8> FormatInsertThreadStreamRequest(const MicromegasTracing::ThreadStream& stream);
