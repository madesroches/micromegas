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

		SpanMetadata(const char* name,
			const char* target,
			const char* file,
			uint32 line)
			: Name(name)
			, Target(target)
			, File(file)
			, Line(line)
		{
		}
	};

	struct BeginThreadSpanEvent
	{
		const SpanMetadata* Desc;
		uint64 Timestamp;

		BeginThreadSpanEvent(const SpanMetadata* desc, uint64 timestamp)
			: Desc(desc)
			, Timestamp(timestamp)
		{
		}
	};

	struct EndThreadSpanEvent
	{
		const SpanMetadata* Desc;
		uint64 Timestamp;

		EndThreadSpanEvent(const SpanMetadata* desc, uint64 timestamp)
			: Desc(desc)
			, Timestamp(timestamp)
		{
		}
	};

} // namespace MicromegasTracing
