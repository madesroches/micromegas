#pragma once
//
//  MicromegasTracing/Macros.h
//
#include "HAL/PlatformTime.h"
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/LogEvents.h"
#include "MicromegasTracing/MetricEvents.h"
#include "MicromegasTracing/SpanEvents.h"

#define MICROMEGAS_LOG_STATIC(target, level, msg)                                                                                   \
	static const MicromegasTracing::LogMetadata PREPROCESSOR_JOIN(logMeta, __LINE__)(level, target, msg, __FILE__, __LINE__); \
	MicromegasTracing::LogStaticStr(MicromegasTracing::LogStaticStrEvent(&PREPROCESSOR_JOIN(logMeta, __LINE__), FPlatformTime::Cycles64()))

#define MICROMEGAS_IMETRIC(target, level, name, unit, expr)                                                                                      \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::IntMetric(MicromegasTracing::IntegerMetricEvent(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64()))

#define MICROMEGAS_FMETRIC(target, level, name, unit, expr)                                                                                      \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::FloatMetric(MicromegasTracing::FloatMetricEvent(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64()))

namespace MicromegasTracing
{
	CORE_API void LogStaticStr(const LogStaticStrEvent& event);
	CORE_API void IntMetric(const IntegerMetricEvent& event);
	CORE_API void FloatMetric(const FloatMetricEvent& event);
	CORE_API void BeginScope(const BeginThreadSpanEvent& event);
	CORE_API void EndScope(const EndThreadSpanEvent& event);

	struct SpanGuard
	{
		const SpanMetadata* Desc;
		explicit SpanGuard(const SpanMetadata* desc)
			: Desc(desc)
		{
			BeginScope(BeginThreadSpanEvent(desc, FPlatformTime::Cycles64()));
		}

		~SpanGuard()
		{
			EndScope(EndThreadSpanEvent(Desc, FPlatformTime::Cycles64()));
		}
	};
} // namespace MicromegasTracing

#define MICROMEGAS_SPAN_SCOPE(target, name)                                                                                     \
	static const MicromegasTracing::SpanMetadata PREPROCESSOR_JOIN(spanMeta, __LINE__)(name, target, __FILE__, __LINE__); \
	MicromegasTracing::SpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__))

#define MICROMEGAS_SPAN_FUNCTION(target) MICROMEGAS_SPAN_SCOPE(target, __FUNCTION__)
