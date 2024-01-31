#pragma once
//
//  LgnTracing/Macros.h
//
#include "HAL/PlatformTime.h"
#include "LgnTracing/LogEvents.h"
#include "LgnTracing/MetricEvents.h"
#include "LgnTracing/SpanEvents.h"
#include "LgnTracing/Dispatch.h"

#define LGN_LOG_STATIC(target, level, msg)                                                                   \
    static const LgnTracing::LogMetadata PREPROCESSOR_JOIN(logMeta,__LINE__)( level, target, msg, TEXT(__FILE__),__LINE__ ); \
    LgnTracing::LogStaticStr( LgnTracing::LogStaticStrEvent( &PREPROCESSOR_JOIN(logMeta,__LINE__), FPlatformTime::Cycles64() ) )


#define LGN_IMETRIC(target, level, name, unit, expr)                     \
    static const LgnTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta,__LINE__)(level, name, unit, target, TEXT(__FILE__ ), __LINE__); \
    LgnTracing::IntMetric( LgnTracing::IntegerMetricEvent( &PREPROCESSOR_JOIN(metricMeta,__LINE__), (expr), FPlatformTime::Cycles64() ) )

#define LGN_FMETRIC(target, level, name, unit, expr)                     \
    static const LgnTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta,__LINE__)(level, name, unit, target, TEXT(__FILE__ ), __LINE__); \
    LgnTracing::FloatMetric( LgnTracing::FloatMetricEvent( &PREPROCESSOR_JOIN(metricMeta,__LINE__), (expr), FPlatformTime::Cycles64() ) )


namespace LgnTracing
{
    struct SpanGuard
    {
        const SpanMetadata* Desc;
        explicit SpanGuard(const SpanMetadata* desc)
            :Desc(desc)
        {
            BeginScope(BeginThreadSpanEvent(desc, FPlatformTime::Cycles64()));
        }

        ~SpanGuard()
        {
            EndScope(EndThreadSpanEvent(Desc, FPlatformTime::Cycles64()));
        }
    };
}

#define LGN_SPAN_SCOPE(target, name) \
    static const LgnTracing::SpanMetadata PREPROCESSOR_JOIN(spanMeta,__LINE__)(name, target, TEXT(__FILE__), __LINE__); \
    LgnTracing::SpanGuard PREPROCESSOR_JOIN(spanguard,__LINE__)( &PREPROCESSOR_JOIN(spanMeta,__LINE__) )
