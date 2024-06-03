#pragma once
//
//  MicromegasTelemetrySink/ThreadDependencies.h
//
#include "MicromegasTracing/ThreadMetadata.h"

typedef MicromegasTracing::HeterogeneousQueue<
	MicromegasTracing::StaticStringDependency,
	MicromegasTracing::SpanMetadataDependency,
	MicromegasTracing::SpanLocationDependency>
	ThreadDependenciesQueue;

struct ExtractThreadDependencies
{
	TSet<const void*> Ids;
	ThreadDependenciesQueue Dependencies;

	ExtractThreadDependencies()
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

	void operator()(const MicromegasTracing::SpanMetadata* Desc)
	{
		bool alreadyInSet = false;
		Ids.Add(Desc, &alreadyInSet);
		if (!alreadyInSet)
		{
			(*this)(MicromegasTracing::StaticStringRef(Desc->Name));
			(*this)(MicromegasTracing::StaticStringRef(Desc->Target));
			(*this)(MicromegasTracing::StaticStringRef(Desc->File));
			Dependencies.Push(MicromegasTracing::SpanMetadataDependency(Desc));
		}
	}

	void operator()(const MicromegasTracing::SpanLocation* Loc)
	{
		bool alreadyInSet = false;
		Ids.Add(Loc, &alreadyInSet);
		if (!alreadyInSet)
		{
			(*this)(MicromegasTracing::StaticStringRef(Loc->Target));
			(*this)(MicromegasTracing::StaticStringRef(Loc->File));
			Dependencies.Push(MicromegasTracing::SpanLocationDependency(Loc));
		}
	}

	void operator()(const MicromegasTracing::BeginThreadSpanEvent& event)
	{
		(*this)(event.Desc);
	}

	void operator()(const MicromegasTracing::EndThreadSpanEvent& event)
	{
		(*this)(event.Desc);
	}

	void operator()(const MicromegasTracing::BeginThreadNamedSpanEvent& event)
	{
		(*this)(event.Desc);
		(*this)(event.Name);
	}

	void operator()(const MicromegasTracing::EndThreadNamedSpanEvent& event)
	{
		(*this)(event.Desc);
		(*this)(event.Name);
	}

	ExtractThreadDependencies(const ExtractThreadDependencies&) = delete;
	ExtractThreadDependencies& operator=(const ExtractThreadDependencies&) = delete;
};
