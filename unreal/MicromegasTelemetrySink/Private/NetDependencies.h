#pragma once
//
//  MicromegasTelemetrySink/NetDependencies.h
//
#include "Containers/Set.h"
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/NetEvents.h"
#include "MicromegasTracing/StaticStringDependency.h"
#include "MicromegasTracing/strings.h"

typedef MicromegasTracing::HeterogeneousQueue<
	MicromegasTracing::StaticStringDependency>
	NetDependenciesQueue;

struct ExtractNetDependencies
{
	TSet<const void*> Ids;
	NetDependenciesQueue Dependencies;

	ExtractNetDependencies()
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

	void operator()(const MicromegasTracing::NetConnectionBeginEvent& Evt) { (*this)(Evt.ConnectionName); }
	void operator()(const MicromegasTracing::NetObjectBeginEvent& Evt) { (*this)(Evt.ObjectName); }
	void operator()(const MicromegasTracing::NetPropertyEvent& Evt) { (*this)(Evt.PropertyName); }
	void operator()(const MicromegasTracing::NetRPCBeginEvent& Evt) { (*this)(Evt.FunctionName); }

	// Events with no string fields
	void operator()(const MicromegasTracing::NetConnectionEndEvent&) {}
	void operator()(const MicromegasTracing::NetObjectEndEvent&) {}
	void operator()(const MicromegasTracing::NetRPCEndEvent&) {}

	ExtractNetDependencies(const ExtractNetDependencies&) = delete;
	ExtractNetDependencies& operator=(const ExtractNetDependencies&) = delete;
};
