#pragma once
//
//  MicromegasTracing/LogMetadata.h
//
#include "MicromegasTracing/QueueMetadata.h"

namespace MicromegasTracing
{
	template <typename T>
	struct GetEventMetadata;

	template <>
	struct GetEventMetadata<TaggedLogInteropEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("TaggedLogInteropEvent"),
				0, // requires custom parsing logic
				false,
				{
					MAKE_UDT_MEMBER_METADATA(TaggedLogInteropEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(TaggedLogInteropEvent, "level", Level, Level, false),
					MAKE_UDT_MEMBER_METADATA(TaggedLogInteropEvent, "target", Target, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(TaggedLogInteropEvent, "properties", Properties, PropertySet*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedLogInteropEvent, "msg", Msg, DynamicString, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<LogMetadataDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("LogMetadataDependency"),
				sizeof(LogMetadataDependency),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "id", Id, uint64, false),
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "target", Target, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "fmt_str", Msg, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "file", File, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "line", Line, uint32, false),
					MAKE_UDT_MEMBER_METADATA(LogMetadataDependency, "level", Level, uint8, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<TaggedLogString>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("TaggedLogString"),
				0, // requires custom parsing logic
				false,
				{
					MAKE_UDT_MEMBER_METADATA(TaggedLogString, "desc", Desc, LogMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedLogString, "properties", Properties, PropertySet*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedLogString, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(TaggedLogString, "msg", Msg, DynamicString, false),
				});
		}
	};
} // namespace MicromegasTracing
