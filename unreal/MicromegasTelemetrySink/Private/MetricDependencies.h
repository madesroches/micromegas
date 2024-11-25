#pragma once
//
//  MicromegasTelemetrySink/MetricDependencies.h
//
#include "MicromegasTracing/MetricEvents.h"
#include "MicromegasTracing/PropertySet.h"
#include "MicromegasTracing/PropertySetDependency.h"

typedef MicromegasTracing::HeterogeneousQueue<
	MicromegasTracing::StaticStringDependency,
	MicromegasTracing::MetricMetadataDependency,
	MicromegasTracing::PropertySetDependency,
	MicromegasTracing::Property>
	MetricDependenciesQueue;

struct ExtractMetricDependencies
{
	TSet<const void*> Ids;
	MetricDependenciesQueue Dependencies;

	ExtractMetricDependencies()
		: Dependencies(1024 * 1024)
	{
	}

	void operator()(const MicromegasTracing::StaticStringRef& str)
	{
		bool alreadyInSet = false;
		Ids.Add(reinterpret_cast<void*>(str.GetID()), &alreadyInSet);
		if (!alreadyInSet)
		{
			Dependencies.Push(MicromegasTracing::StaticStringDependency(str));
		}
	}

	void operator()(const MicromegasTracing::MetricMetadata* metricDesc)
	{
		bool alreadyInSet = false;
		Ids.Add(metricDesc, &alreadyInSet);
		if (!alreadyInSet)
		{
			(*this)(MicromegasTracing::StaticStringRef(metricDesc->Name));
			(*this)(MicromegasTracing::StaticStringRef(metricDesc->Unit));
			(*this)(MicromegasTracing::StaticStringRef(metricDesc->Target));
			(*this)(MicromegasTracing::StaticStringRef(metricDesc->File));
			Dependencies.Push(MicromegasTracing::MetricMetadataDependency(metricDesc));
		}
	}

	void operator()(const MicromegasTracing::PropertySet* Properties)
	{
		bool alreadyInSet = false;
		Ids.Add(Properties, &alreadyInSet);
		if (!alreadyInSet)
		{
			for (const TPair<FName, FName>& Prop : Properties->GetContext())
			{
				(*this)(MicromegasTracing::StaticStringRef(Prop.Key));
				(*this)(MicromegasTracing::StaticStringRef(Prop.Value));
			}
			Dependencies.Push(MicromegasTracing::PropertySetDependency(Properties));
		}
	}

	void operator()(const MicromegasTracing::IntegerMetricEvent& event)
	{
		(*this)(event.Desc);
	}

	void operator()(const MicromegasTracing::FloatMetricEvent& event)
	{
		(*this)(event.Desc);
	}

	void operator()(const MicromegasTracing::TaggedIntegerMetricEvent& event)
	{
		(*this)(event.Desc);
		(*this)(event.Properties);
	}

	void operator()(const MicromegasTracing::TaggedFloatMetricEvent& event)
	{
		(*this)(event.Desc);
		(*this)(event.Properties);
	}

	ExtractMetricDependencies(const ExtractMetricDependencies&) = delete;
	ExtractMetricDependencies& operator=(const ExtractMetricDependencies&) = delete;
};
