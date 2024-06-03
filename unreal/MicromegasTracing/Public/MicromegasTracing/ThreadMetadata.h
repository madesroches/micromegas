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
		const char* Name;
		const char* Target;
		const char* File;
		uint32 Line;

		explicit SpanMetadataDependency(const SpanMetadata* Desc)
			: Id(reinterpret_cast<uint64>(Desc))
			, Name(Desc->Name)
			, Target(Desc->Target)
			, File(Desc->File)
			, Line(Desc->Line)
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

	template <>
	struct GetEventMetadata<BeginThreadNamedSpanEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("BeginThreadNamedSpanEvent"),
				sizeof(BeginThreadNamedSpanEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(BeginThreadNamedSpanEvent, "thread_span_location", Desc, NamedSpanLocation*, true),
					MAKE_UDT_MEMBER_METADATA(BeginThreadNamedSpanEvent, "name", Name, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(BeginThreadNamedSpanEvent, "time", Timestamp, uint64, false) });
		}
	};

	template <>
	struct GetEventMetadata<EndThreadNamedSpanEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("EndThreadNamedSpanEvent"),
				sizeof(EndThreadNamedSpanEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(EndThreadNamedSpanEvent, "thread_span_location", Desc, NamedSpanLocation*, true),
					MAKE_UDT_MEMBER_METADATA(EndThreadNamedSpanEvent, "name", Name, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(EndThreadNamedSpanEvent, "time", Timestamp, uint64, false) });
		}
	};

	struct SpanLocationDependency
	{
		uint64 Id;
		const char* Target;
		const char* File;
		uint32 Line;

		explicit SpanLocationDependency(const SpanLocation* Desc)
			: Id(reinterpret_cast<uint64>(Desc))
			, Target(Desc->Target)
			, File(Desc->File)
			, Line(Desc->Line)
		{
		}
	};

	template <>
	struct GetEventMetadata<SpanLocationDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("SpanLocationDependency"),
				sizeof(SpanLocationDependency),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(SpanLocationDependency, "id", Id, uint64, false),
					MAKE_UDT_MEMBER_METADATA(SpanLocationDependency, "target", Target, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(SpanLocationDependency, "file", File, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(SpanLocationDependency, "line", Line, uint32, false),
				});
		}
	};

} // namespace MicromegasTracing
