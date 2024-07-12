#pragma once

#include "Async/Mutex.h"
#include "Containers/RingBuffer.h"
#include "HAL/IConsoleManager.h"
#include "MicromegasTelemetrySink/FlushMonitor.h"
#include "MicromegasTracing/Fwd.h"
#include "Misc/DateTime.h"
#include "RunningAverage.h"
#include "Templates/UniquePtr.h"

using TimeRange = TTuple<FDateTime, FDateTime>; // (begin, end)

class FSamplingController
{
public:
	explicit FSamplingController(const SharedFlushMonitor& FlushMonitor);
	~FSamplingController();

	void Tick();

	bool ShouldSampleBlock(const MicromegasTracing::LogBlockPtr& Block) const;
	bool ShouldSampleBlock(const MicromegasTracing::MetricsBlockPtr& Block) const;
	bool ShouldSampleBlock(const MicromegasTracing::ThreadBlockPtr& Block) const;

private:
	SharedFlushMonitor FlushMonitor;
	FRunningAverage FrameTimeRunningAvg;
	FDateTime LastFrameDateTime;
	double SpikeFactor;
	
	mutable UE::FMutex SampledTimeRangesMutex;
	TRingBuffer<TimeRange> SampledTimeRanges;
	

	TUniquePtr<TAutoConsoleVariable<bool>> CVarLogEnable;
	TUniquePtr<TAutoConsoleVariable<bool>> CVarMetricsEnable;
	TUniquePtr<TAutoConsoleVariable<bool>> CVarSpansEnable;
	TUniquePtr<TAutoConsoleVariable<bool>> CVarSpansAll;
};

typedef TSharedPtr<FSamplingController, ESPMode::ThreadSafe> SharedSamplingController;
