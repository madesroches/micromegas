#pragma once
//
//  MicromegasTracing/ThreadMetadata.h
//
#include "MicromegasTracing/QueueMetadata.h"

namespace MicromegasTracing
{
	template <>
	struct GetEventMetadata<BeginThreadSpanEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("BeginThreadSpanEvent"),
				sizeof(BeginThreadSpanEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(BeginThreadSpanEvent, "thread_span_desc", Desc, SpanMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(BeginThreadSpanEvent, "time", Timestamp, uint64, false) });
		}
	};

	template <>
	struct GetEventMetadata<EndThreadSpanEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("EndThreadSpanEvent"),
				sizeof(EndThreadSpanEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(EndThreadSpanEvent, "thread_span_desc", Desc, SpanMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(EndThreadSpanEvent, "time", Timestamp, uint64, false) });
		}
	};

	struct SpanMetadataDependency
	{
		uint64 Id;
		const TCHAR* Name;
		const TCHAR* Target;
		const TCHAR* File;
		uint32 Line;

		explicit SpanMetadataDependency(const SpanMetadata* desc)
			: Id(reinterpret_cast<uint64>(desc))
			, Name(desc->Name)
			, Target(desc->Target)
			, File(desc->File)
			, Line(desc->Line)
		{
		}
	};

	template <>
	struct GetEventMetadata<SpanMetadataDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("SpanMetadataDependency"),
				sizeof(SpanMetadataDependency),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(SpanMetadataDependency, "id", Id, uint64, false),
					MAKE_UDT_MEMBER_METADATA(SpanMetadataDependency, "name", Name, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(SpanMetadataDependency, "target", Target, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(SpanMetadataDependency, "file", File, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(SpanMetadataDependency, "line", Line, uint32, false),
				});
		}
	};

} // namespace MicromegasTracing
