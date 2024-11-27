#pragma once
//
//  MicromegasTracing/MetricEvents.h
//
#include "MicromegasTracing/Verbosity.h"

namespace MicromegasTracing
{
	class PropertySet;

	struct MetricMetadata
	{
		Verbosity::Type Lod;
		const TCHAR* Name;
		const TCHAR* Unit;
		const char* Target;
		const char* File;
		uint32 Line;

		MetricMetadata(Verbosity::Type lod,
			const TCHAR* name,
			const TCHAR* unit,
			const char* target,
			const char* file,
			uint32 line)
			: Lod(lod)
			, Name(name)
			, Unit(unit)
			, Target(target)
			, File(file)
			, Line(line)
		{
		}
	};

	// IntegerMetricEvent is deprecated, use TaggedIntegerMetricEvent
	struct IntegerMetricEvent
	{
		const MetricMetadata* Desc;
		uint64 Value;
		uint64 Timestamp;

		IntegerMetricEvent(const MetricMetadata* desc, uint64 value, uint64 timestamp)
			: Desc(desc)
			, Value(value)
			, Timestamp(timestamp)
		{
		}
	};

	// FloatMetricEvent is deprecated, use TaggedFloatMetricEvent
	struct FloatMetricEvent
	{
		const MetricMetadata* Desc;
		double Value;
		uint64 Timestamp;

		FloatMetricEvent(const MetricMetadata* desc, double value, uint64 timestamp)
			: Desc(desc)
			, Value(value)
			, Timestamp(timestamp)
		{
		}
	};

	struct TaggedIntegerMetricEvent
	{
		const MetricMetadata* Desc;
		const PropertySet* Properties;
		uint64 Value;
		uint64 Timestamp;

		TaggedIntegerMetricEvent(const MetricMetadata* desc, const PropertySet* properties, uint64 value, uint64 timestamp)
			: Desc(desc)
			, Properties(properties)
			, Value(value)
			, Timestamp(timestamp)
		{
		}
	};

	struct TaggedFloatMetricEvent
	{
		const MetricMetadata* Desc;
		const PropertySet* Properties;
		double Value;
		uint64 Timestamp;

		TaggedFloatMetricEvent(const MetricMetadata* desc, const PropertySet* properties, double value, uint64 timestamp)
			: Desc(desc)
			, Properties(properties)
			, Value(value)
			, Timestamp(timestamp)
		{
		}
	};

	struct MetricMetadataDependency
	{
		uint64 Id;
		Verbosity::Type Lod;
		const TCHAR* Name;
		const TCHAR* Unit;
		const char* Target;
		const char* File;
		uint32 Line;

		explicit MetricMetadataDependency(const MetricMetadata* mm)
			: Id(reinterpret_cast<uint64>(mm))
			, Lod(mm->Lod)
			, Name(mm->Name)
			, Unit(mm->Unit)
			, Target(mm->Target)
			, File(mm->File)
			, Line(mm->Line)
		{
		}
	};
} // namespace MicromegasTracing
