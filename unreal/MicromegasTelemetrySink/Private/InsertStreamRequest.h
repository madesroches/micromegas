#pragma once
//
//  MicromegasTelemetrySink/InsertStreamRequest.h
//
#include "MicromegasTracing/Fwd.h"

FString FormatInsertLogStreamRequest( const MicromegasTracing::LogStream& stream );
FString FormatInsertMetricStreamRequest( const MicromegasTracing::MetricStream& stream );
FString FormatInsertThreadStreamRequest( const MicromegasTracing::ThreadStream& stream );
