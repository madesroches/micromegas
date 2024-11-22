#pragma once
//
//  MicromegasTracing/Macros.h
//
#include "HAL/PlatformTime.h"
#include "Misc/Optional.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/LogEvents.h"
#include "MicromegasTracing/SpanEvents.h"

#define MICROMEGAS_LOG_STATIC(target, level, msg)                                                                             \
	static const MicromegasTracing::LogMetadata PREPROCESSOR_JOIN(logMeta, __LINE__)(level, target, msg, __FILE__, __LINE__); \
	MicromegasTracing::Dispatch::LogStaticStr(MicromegasTracing::LogStaticStrEvent(&PREPROCESSOR_JOIN(logMeta, __LINE__), FPlatformTime::Cycles64()))

// we could do like the rust instrumentation and log additional metadata instead of relying on the LogStringInteropEvent
#define MICROMEGAS_LOG_DYNAMIC(target, level, msg) \
	MicromegasTracing::Dispatch::LogInterop(MicromegasTracing::LogStringInteropEvent(FPlatformTime::Cycles64(), level, MicromegasTracing::StaticStringRef(target), MicromegasTracing::DynamicString(msg)))

#define MICROMEGAS_IMETRIC(target, level, name, unit, expr)                                                                                \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::Dispatch::IntMetric(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64())

#define MICROMEGAS_FMETRIC(target, level, name, unit, expr)                                                                                \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::Dispatch::FloatMetric(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64())

namespace MicromegasTracing
{
	struct SpanGuard
	{
		const SpanMetadata* Desc;
		explicit SpanGuard(const SpanMetadata* desc)
			: Desc(desc)
		{
			Dispatch::BeginScope(BeginThreadSpanEvent(desc, FPlatformTime::Cycles64()));
		}

		~SpanGuard()
		{
			Dispatch::EndScope(EndThreadSpanEvent(Desc, FPlatformTime::Cycles64()));
		}
	};

	struct NamedSpanGuard
	{
		const SpanLocation* Desc;
		TOptional<const StaticStringRef> Name;

		NamedSpanGuard(const SpanLocation* InDesc, const TOptional<const StaticStringRef>& InName)
			: Desc(InDesc)
			, Name(InName)
		{
			if (Name.IsSet())
			{
				Dispatch::BeginNamedSpan(BeginThreadNamedSpanEvent(Desc, FPlatformTime::Cycles64(), *Name));
			}
		}

		~NamedSpanGuard()
		{
			if (Name.IsSet())
			{
				Dispatch::EndNamedSpan(EndThreadNamedSpanEvent(Desc, FPlatformTime::Cycles64(), *Name));
			}
		}
	};

} // namespace MicromegasTracing

// MICROMEGAS_SPAN_SCOPE: the specified name is part of the scope metadata - it can't be changed from one call to the next
#define MICROMEGAS_SPAN_SCOPE(target, name)                                                                               \
	static const MicromegasTracing::SpanMetadata PREPROCESSOR_JOIN(spanMeta, __LINE__)(name, target, __FILE__, __LINE__); \
	MicromegasTracing::SpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__))

// MICROMEGAS_SPAN_NAME: the specified name can be variable, but any specified variable must have a static lifetime
#define MICROMEGAS_SPAN_NAME(target, name)                                                                          \
	static const MicromegasTracing::SpanLocation PREPROCESSOR_JOIN(spanMeta, __LINE__)(target, __FILE__, __LINE__); \
	MicromegasTracing::NamedSpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__), MicromegasTracing::StaticStringRef((name)))

#define MICROMEGAS_SPAN_NAME_CONDITIONAL(target, cond, name)                                                        \
	static const MicromegasTracing::SpanLocation PREPROCESSOR_JOIN(spanMeta, __LINE__)(target, __FILE__, __LINE__); \
	MicromegasTracing::NamedSpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__), (cond) ? MicromegasTracing::StaticStringRef((name)) : TOptional<const MicromegasTracing::StaticStringRef>())

// MICROMEGAS_SPAN_UOBJECT: the specified object shall implement a method `GetFName` with a return type that is
//                          compatible with StaticStringRef
#define MICROMEGAS_SPAN_UOBJECT(target, object)                                                                     \
	static const MicromegasTracing::SpanLocation PREPROCESSOR_JOIN(spanMeta, __LINE__)(target, __FILE__, __LINE__); \
	MicromegasTracing::NamedSpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__), MicromegasTracing::StaticStringRef((object)->GetFName()))

#define MICROMEGAS_SPAN_UOBJECT_CONDITIONAL(target, cond, object)                                                   \
	static const MicromegasTracing::SpanLocation PREPROCESSOR_JOIN(spanMeta, __LINE__)(target, __FILE__, __LINE__); \
	MicromegasTracing::NamedSpanGuard PREPROCESSOR_JOIN(spanguard, __LINE__)(&PREPROCESSOR_JOIN(spanMeta, __LINE__), (cond) ? MicromegasTracing::StaticStringRef((object)->GetFName()) : TOptional<const MicromegasTracing::StaticStringRef>())

#if !defined(__clang__)
	#define MICROMEGAS_FUNCTION_NAME __FUNCTION__
#else
	#define MICROMEGAS_FUNCTION_NAME __PRETTY_FUNCTION__
#endif

#define MICROMEGAS_SPAN_FUNCTION(target) MICROMEGAS_SPAN_SCOPE(target, MICROMEGAS_FUNCTION_NAME)
