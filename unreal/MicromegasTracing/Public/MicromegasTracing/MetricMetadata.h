#pragma once
//
//  MicromegasTracing/MetricMetadata.h
//
#include "MicromegasTracing/MetricEvents.h"

namespace MicromegasTracing
{
	template <>
	struct GetEventMetadata<TaggedIntegerMetricEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("TaggedIntegerMetricEvent"),
				sizeof(TaggedIntegerMetricEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(TaggedIntegerMetricEvent, "desc", Desc, MetricMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedIntegerMetricEvent, "properties", Properties, PropertySet*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedIntegerMetricEvent, "value", Value, uint64, false),
					MAKE_UDT_MEMBER_METADATA(TaggedIntegerMetricEvent, "time", Timestamp, uint64, false) });
		}
	};

	template <>
	struct GetEventMetadata<TaggedFloatMetricEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("TaggedFloatMetricEvent"),
				sizeof(TaggedFloatMetricEvent),
				false,
				{ MAKE_UDT_MEMBER_METADATA(TaggedFloatMetricEvent, "desc", Desc, MetricMetadata*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedFloatMetricEvent, "properties", Properties, PropertySet*, true),
					MAKE_UDT_MEMBER_METADATA(TaggedFloatMetricEvent, "value", Value, f64, false),
					MAKE_UDT_MEMBER_METADATA(TaggedFloatMetricEvent, "time", Timestamp, uint64, false) });
		}
	};

	template <>
	struct GetEventMetadata<MetricMetadataDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("MetricMetadataDependency"),
				sizeof(MetricMetadataDependency),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "id", Id, uint64, false),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "lod", Lod, uint8, false),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "name", Name, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "unit", Unit, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "target", Target, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "file", File, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(MetricMetadataDependency, "line", Line, uint32, false),
				});
		}
	};
} // namespace MicromegasTracing
