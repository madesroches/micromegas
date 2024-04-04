#pragma once
//
//  MicromegasTracing/LogMetadata.h
//
namespace MicromegasTracing
{
	template <typename T>
	struct GetEventMetadata;

	template <>
	struct GetEventMetadata<LogStringInteropEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("LogStringInteropEventV3"),
				0, // requires custom parsing logic
				false,
				{});
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
	struct GetEventMetadata<LogStaticStrEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("LogStaticStrEvent"),
				sizeof(LogStaticStrEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(LogStaticStrEvent, "desc", Desc, LogMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(LogStaticStrEvent, "time", Timestamp, uint64, false),
				});
		}
	};
} // namespace MicromegasTracing
