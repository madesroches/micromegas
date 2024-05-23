#pragma once

#include "HAL/IConsoleManager.h"
#include "MicromegasTracing/Fwd.h"
#include "Templates/UniquePtr.h"

class FSamplingController
{
public:
	FSamplingController();

	bool ShouldSampleBlock(const MicromegasTracing::LogBlockPtr& Block) const;
	bool ShouldSampleBlock(const MicromegasTracing::MetricsBlockPtr& Block) const;
	bool ShouldSampleBlock(const MicromegasTracing::ThreadBlockPtr& Block) const;

private:
	TUniquePtr<TAutoConsoleVariable<bool>> CVarLogEnable;
	TUniquePtr<TAutoConsoleVariable<bool>> CVarMetricsEnable;
	TUniquePtr<TAutoConsoleVariable<bool>> CVarSpansEnable;
};

typedef TSharedPtr<FSamplingController, ESPMode::ThreadSafe> SharedSamplingController;
