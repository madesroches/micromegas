#pragma once
//
//  LgnTelemetrySink/InsertStreamRequest.h
//
#include "LgnTracing/Fwd.h"

FString FormatInsertLogStreamRequest( const LgnTracing::LogStream& stream );
FString FormatInsertMetricStreamRequest( const LgnTracing::MetricStream& stream );
FString FormatInsertThreadStreamRequest( const LgnTracing::ThreadStream& stream );
