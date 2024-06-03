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

	void operator()(const MicromegasTracing::StaticStringRef& Str)
	{
		bool alreadyInSet = false;
		Ids.Add(reinterpret_cast<void*>(Str.GetID()), &alreadyInSet);
		if (!alreadyInSet)
		{
			Dependencies.Push(MicromegasTracing::StaticStringDependency(Str));
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

	void operator()(const MicromegasTracing::BeginThreadSpanEvent& Event)
	{
		(*this)(Event.Desc);
	}

	void operator()(const MicromegasTracing::EndThreadSpanEvent& Event)
	{
		(*this)(Event.Desc);
	}

	void operator()(const MicromegasTracing::BeginThreadNamedSpanEvent& Event)
	{
		(*this)(Event.Desc);
		(*this)(Event.Name);
	}

	void operator()(const MicromegasTracing::EndThreadNamedSpanEvent& Event)
	{
		(*this)(Event.Desc);
		(*this)(Event.Name);
	}

	ExtractThreadDependencies(const ExtractThreadDependencies&) = delete;
	ExtractThreadDependencies& operator=(const ExtractThreadDependencies&) = delete;
};
