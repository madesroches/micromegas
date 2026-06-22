#pragma once
//
//  MicromegasTelemetrySink/ImageDependencies.h
//
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/ImageEvents.h"
#include "MicromegasTracing/StaticStringDependency.h"

typedef MicromegasTracing::HeterogeneousQueue<
	MicromegasTracing::StaticStringDependency>
	ImageDependenciesQueue;

struct ExtractImageDependencies
{
	ImageDependenciesQueue Dependencies;

	ExtractImageDependencies()
		: Dependencies(1024)
	{
	}

	void operator()(const MicromegasTracing::ImageEvent&) {}
};
