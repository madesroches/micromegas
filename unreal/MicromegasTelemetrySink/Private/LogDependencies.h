#pragma once
//
//  MicromegasTelemetrySink/LogDependencies.h
//
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/strings.h"
#include "MicromegasTracing/LogEvents.h"
#include "MicromegasTracing/StaticStringDependency.h"
#include "Containers/Set.h"

typedef MicromegasTracing::HeterogeneousQueue<MicromegasTracing::StaticStringDependency, MicromegasTracing::LogMetadataDependency> LogDependenciesQueue;

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

	void operator()(const MicromegasTracing::LogStringInteropEvent& evt)
	{
		(*this)(evt.Target);
	}

	void operator()(const MicromegasTracing::LogStaticStrEvent& evt)
	{
		(*this)(evt.Desc);
	}

	ExtractLogDependencies(const ExtractLogDependencies&) = delete;
	ExtractLogDependencies& operator=(const ExtractLogDependencies&) = delete;
};
