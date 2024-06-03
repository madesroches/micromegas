#pragma once
//
//  MicromegasTracing/SpanEvents.h
//
namespace MicromegasTracing
{
	struct SpanMetadata
	{
		const char* Name;
		const char* Target;
		const char* File;
		uint32 Line;

		SpanMetadata(const char* InName,
			const char* InTarget,
			const char* InFile,
			uint32 InLine)
			: Name(InName)
			, Target(InTarget)
			, File(InFile)
			, Line(InLine)
		{
		}
	};

	struct BeginThreadSpanEvent
	{
		const SpanMetadata* Desc;
		uint64 Timestamp;

		BeginThreadSpanEvent(const SpanMetadata* InDesc, uint64 InTimestamp)
			: Desc(InDesc)
			, Timestamp(InTimestamp)
		{
		}
	};

	struct EndThreadSpanEvent
	{
		const SpanMetadata* Desc;
		uint64 Timestamp;

		EndThreadSpanEvent(const SpanMetadata* InDesc, uint64 InTimestamp)
			: Desc(InDesc)
			, Timestamp(InTimestamp)
		{
		}
	};

	struct SpanLocation
	{
		const char* Target;
		const char* File;
		uint32 Line;

		SpanLocation(const char* InTarget,
			const char* InFile,
			uint32 InLine)
			: Target(InTarget)
			, File(InFile)
			, Line(InLine)
		{
		}
	};

	struct BeginThreadNamedSpanEvent
	{
		const SpanLocation* Desc;
		uint64 Timestamp;
		StaticStringRef Name;

		BeginThreadNamedSpanEvent(const SpanLocation* InDesc, uint64 InTimestamp, const StaticStringRef& InName)
			: Desc(InDesc)
			, Timestamp(InTimestamp)
			, Name(InName)
		{
		}
	};

	struct EndThreadNamedSpanEvent
	{
		const SpanLocation* Desc;
		uint64 Timestamp;
		StaticStringRef Name;

		EndThreadNamedSpanEvent(const SpanLocation* InDesc, uint64 InTimestamp, const StaticStringRef& InName)
			: Desc(InDesc)
			, Timestamp(InTimestamp)
			, Name(InName)
		{
		}
	};

} // namespace MicromegasTracing
