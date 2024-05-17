#include "SamplingController.h"

FSamplingController::FSamplingController()
	: CVarLogEnable(new TAutoConsoleVariable<bool>(
		TEXT("telemetry.log.enable"),
		true,
		TEXT("Record the process's log")))
	, CVarMetricsEnable(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.metrics.enable"),
		  true,
		  TEXT("Record the frame metrics")))
	, CVarSpansEnable(new TAutoConsoleVariable<bool>(
		  TEXT("telemetry.spans.enable"),
		  false,
		  TEXT("Record the cpu trace from all threads")))
{
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
	return CVarSpansEnable->GetValueOnAnyThread();
}
