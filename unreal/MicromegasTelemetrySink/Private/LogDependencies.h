#pragma once
//
//  MicromegasTelemetrySink/LogDependencies.h
//
#include "Containers/Set.h"
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/LogEvents.h"
#include "MicromegasTracing/PropertySet.h"
#include "MicromegasTracing/PropertySetDependency.h"
#include "MicromegasTracing/StaticStringDependency.h"
#include "MicromegasTracing/strings.h"

typedef MicromegasTracing::HeterogeneousQueue<
	MicromegasTracing::StaticStringDependency,
	MicromegasTracing::LogMetadataDependency,
	MicromegasTracing::PropertySetDependency,
	MicromegasTracing::Property>
	LogDependenciesQueue;

struct ExtractLogDependencies
{
	TSet<const void*> Ids;
	LogDependenciesQueue Dependencies;

	ExtractLogDependencies()
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

	void operator()(const MicromegasTracing::LogMetadata* logDesc)
	{
		bool alreadyInSet = false;
		Ids.Add(logDesc, &alreadyInSet);
		if (!alreadyInSet)
		{
			(*this)(MicromegasTracing::StaticStringRef(logDesc->Target));
			(*this)(MicromegasTracing::StaticStringRef(logDesc->Msg));
			(*this)(MicromegasTracing::StaticStringRef(logDesc->File));
			Dependencies.Push(MicromegasTracing::LogMetadataDependency(logDesc));
		}
	}

	void operator()(const MicromegasTracing::TaggedLogInteropEvent& evt)
	{
		(*this)(evt.Target);
		(*this)(evt.Properties);
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

	void operator()(const MicromegasTracing::TaggedLogString& evt)
	{
		(*this)(evt.Desc);
		(*this)(evt.Properties);
	}

	ExtractLogDependencies(const ExtractLogDependencies&) = delete;
	ExtractLogDependencies& operator=(const ExtractLogDependencies&) = delete;
};
