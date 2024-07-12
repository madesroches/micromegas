#include "SamplingController.h"
#include "Async/UniqueLock.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/EventBlock.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/App.h"
#include "Misc/CoreDelegates.h"

#if WITH_EDITOR
constexpr bool SPANS_SAMPLING_ENABLED_BY_DEFAULT = false;
#else
constexpr bool SPANS_SAMPLING_ENABLED_BY_DEFAULT = true;
#endif

constexpr size_t RUNNING_AVERAGE_WINDOW_SIZE = 32;
constexpr double RUNNING_AVERAGE_INITIAL_VALUE = 60.0;
constexpr double INITIAL_SPIKE_FACTOR = 1.3;
constexpr double SPIKE_FACTOR_INFLATION = 1.01;

FString FormatTimeRange(const TimeRange& r)
{
	const TCHAR* Format = TEXT("%M:%S.%s");
	return FString::Printf(TEXT("[%s %s]"), *r.Get<0>().ToString(Format), *r.Get<1>().ToString(Format));
}

bool TimeRangesOverlap(const TimeRange& Lhs, const TimeRange& Rhs)
{
	check(Lhs.Get<0>() <= Lhs.Get<1>());
	check(Rhs.Get<0>() <= Rhs.Get<1>());
	return Lhs.Get<0>() <= Rhs.Get<1>() && Lhs.Get<1>() >= Rhs.Get<0>();
}

FSamplingController::FSamplingController(const SharedFlushMonitor& InFlushMonitor)
	: FlushMonitor(InFlushMonitor)
	, FrameTimeRunningAvg(RUNNING_AVERAGE_WINDOW_SIZE, RUNNING_AVERAGE_INITIAL_VALUE) // init with a large value to avoid triggering on the first frames, but not so large to get into numerical instability
	, LastFrameDateTime(FDateTime::UtcNow())
	, SpikeFactor(INITIAL_SPIKE_FACTOR)
	, CVarLogEnable(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.log.enable"),
		  true,
		  TEXT("Record the process's log")))
	, CVarMetricsEnable(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.metrics.enable"),
		  true,
		  TEXT("Record the frame metrics")))
	, CVarSpansEnable(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.spans.enable"),
		  SPANS_SAMPLING_ENABLED_BY_DEFAULT,
		  TEXT("Allow sampling the cpu spans")))
	, CVarSpansAll(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.spans.all"),
		  false,
		  TEXT("Always send all spans - uses significant bandwidth")))
{
	FCoreDelegates::OnBeginFrame.AddRaw(this, &FSamplingController::Tick);
}

FSamplingController::~FSamplingController()
{
	FCoreDelegates::OnBeginFrame.RemoveAll(this);
}

void FSamplingController::Tick()
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (!CVarSpansEnable->GetValueOnAnyThread())
	{
		return;
	}

	// A spike is detected when the last frame time exceeds the running average multipled by SpikeFactor
	FDateTime Now = FDateTime::UtcNow();
	double LastFrameDeltaTime = FApp::GetDeltaTime(); // we could compute it, but I prefer to rely on the same number that is fed as a metric
	FrameTimeRunningAvg.Add(LastFrameDeltaTime);

	double RunningAvg = FrameTimeRunningAvg.Get();
	if (LastFrameDeltaTime >= RunningAvg * SpikeFactor)
	{
		FDateTime SampleExpiration = Now - FTimespan(static_cast<int64>(FlushMonitor->GetFlushPeriodSeconds() * ETimespan::TicksPerSecond));
		TimeRange NewRange(LastFrameDateTime, Now);
		UE_LOG(LogMicromegasTelemetrySink, Verbose, TEXT("Spike detected: range=%s factor=%f delta=%f RunningAvg=%f"), *FormatTimeRange(NewRange), SpikeFactor, LastFrameDeltaTime, RunningAvg);

		UE::TUniqueLock<UE::FMutex> Lock(SampledTimeRangesMutex);
		SampledTimeRanges.Add(NewRange);
		// check for out of date samples only when adding a new one to avoid acquiring the lock every frame
		while (!SampledTimeRanges.IsEmpty() && SampledTimeRanges[0].Get<1>() < SampleExpiration)
		{
			SampledTimeRanges.PopFront();
		}
		SpikeFactor *= SPIKE_FACTOR_INFLATION; // making the spike detector less sensitive as we collect spikes
	}

	LastFrameDateTime = Now;
}

bool FSamplingController::ShouldSampleBlock(const MicromegasTracing::LogBlockPtr& Block) const
{
	return CVarLogEnable->GetValueOnAnyThread();
}

bool FSamplingController::ShouldSampleBlock(const MicromegasTracing::MetricsBlockPtr& Block) const
{
	return CVarMetricsEnable->GetValueOnAnyThread();
}

bool FSamplingController::ShouldSampleBlock(const MicromegasTracing::ThreadBlockPtr& Block) const
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	if (CVarSpansAll->GetValueOnAnyThread())
	{
		return true;
	}
	if (!CVarSpansEnable->GetValueOnAnyThread())
	{
		return false;
	}
	const TimeRange BlockRange(Block->GetBeginTime().SystemTime, Block->GetEndTime().SystemTime);
	UE::TUniqueLock<UE::FMutex> Lock(SampledTimeRangesMutex);
	for (const TimeRange& Sample : SampledTimeRanges)
	{
		if (TimeRangesOverlap(BlockRange, Sample))
		{
			return true;
		}
	}
	return false;
}
