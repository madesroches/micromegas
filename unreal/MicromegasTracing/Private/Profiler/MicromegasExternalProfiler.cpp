//
//  MicromegasTracing/Profiler/MicromegasExternalProfiler.cpp
//
#include "CoreTypes.h"
#include "ProfilingDebugging/ExternalProfiler.h"
#include "Features/IModularFeatures.h"
#include "Math/Color.h"
#include "HAL/PlatformTime.h"
#include "Logging/LogMacros.h"
#include "Containers/Array.h"
#include "Templates/UniquePtr.h"
#include "UObject/NameTypes.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/SpanEvents.h"
#include "MicromegasTracing/strings.h"

#if UE_EXTERNAL_PROFILING_ENABLED

DEFINE_LOG_CATEGORY_STATIC(LogMicromegasProfiler, Log, All);

namespace MicromegasTracing {

CORE_API extern const TCHAR MicromegasProfilerName[] = TEXT("Micromegas");

/**
* Micromegas implementation of FExternalProfiler
*/
class FMicromegasExternalProfiler : public FExternalProfiler
{
public:
	FMicromegasExternalProfiler();
	~FMicromegasExternalProfiler() override;

	//~ Begin FExternalProfiler interface
	const TCHAR* GetProfilerName() const final;
	void FrameSync() final;
	void ProfilerPauseFunction() final;
	void ProfilerResumeFunction() final;
	void Register() final;
	void StartScopedEvent(const FColor& Color, const TCHAR* Text) final;
	void StartScopedEvent(const FColor& Color, const ANSICHAR* Text) final;
	void EndScopedEvent() final;
	//~ End FExternalProfiler interface

private:
	using FProfilerNamedSpanTag = Dispatch::FProfilerNamedSpanTag;
	void StartScopedEventImpl(const StaticStringRef& Name);
	void EndScopedEventImpl();

	static const SpanLocation MicromegasProfilerSpanLoc;

	// Per-thread stack of active span names, used to pair each EndScopedEvent with its BeginScopedEvent
	static thread_local TArray<StaticStringRef> ScopeStack;
};

const SpanLocation FMicromegasExternalProfiler::MicromegasProfilerSpanLoc("ExternalProfiler", __FILE__, __LINE__);
thread_local TArray<StaticStringRef> FMicromegasExternalProfiler::ScopeStack;

FMicromegasExternalProfiler::FMicromegasExternalProfiler()
{
	// Register as a modular feature
	IModularFeatures::Get().RegisterModularFeature(FExternalProfiler::GetFeatureName(), this);
}

FMicromegasExternalProfiler::~FMicromegasExternalProfiler()
{
	IModularFeatures::Get().UnregisterModularFeature(FExternalProfiler::GetFeatureName(), this);
}

const TCHAR* FMicromegasExternalProfiler::GetProfilerName() const
{
	return MicromegasProfilerName;
}

void FMicromegasExternalProfiler::FrameSync()
{
}

void FMicromegasExternalProfiler::ProfilerPauseFunction()
{
}

void FMicromegasExternalProfiler::ProfilerResumeFunction()
{
}

void FMicromegasExternalProfiler::Register()
{
	if (GDispatch == nullptr)
	{
		Dispatch::bProfilerEnabled.store(false, std::memory_order_relaxed);
		UE_LOG(LogMicromegasProfiler, Warning,
			TEXT("Micromegas external profiler selected but telemetry is not initialized; scoped events will be dropped."));
	}
	else
	{
		Dispatch::bProfilerEnabled.store(true, std::memory_order_relaxed);
	}
}

void FMicromegasExternalProfiler::StartScopedEvent(const FColor& /*Color*/, const TCHAR* Text)
{
	StartScopedEventImpl(StaticStringRef(FName(Text)));
}

void FMicromegasExternalProfiler::StartScopedEvent(const FColor& /*Color*/, const ANSICHAR* Text)
{
	StartScopedEventImpl(StaticStringRef(FName(Text)));
}

FORCEINLINE void FMicromegasExternalProfiler::StartScopedEventImpl(const StaticStringRef& Name)
{
	ScopeStack.Push(Name);
	Dispatch::BeginNamedSpan(
		BeginThreadNamedSpanEvent(&MicromegasProfilerSpanLoc, FPlatformTime::Cycles64(), Name), FProfilerNamedSpanTag{});
}

FORCEINLINE void FMicromegasExternalProfiler::EndScopedEvent()
{
	EndScopedEventImpl();
}

void FMicromegasExternalProfiler::EndScopedEventImpl()
{
	if (ScopeStack.Num() > 0)
	{
		const StaticStringRef Name = ScopeStack.Pop(EAllowShrinking::No);
		Dispatch::EndNamedSpan(
			EndThreadNamedSpanEvent(&MicromegasProfilerSpanLoc, FPlatformTime::Cycles64(), Name), FProfilerNamedSpanTag{});
	}
}

namespace MicromegasProfilerPrivate {

struct FAtModuleInit
{
	FAtModuleInit()
	{
		static TUniquePtr<FMicromegasExternalProfiler> Profiler = MakeUnique<FMicromegasExternalProfiler>();
	}
};

static FAtModuleInit AtModuleInit;

} // namespace MicromegasProfilerPrivate
} // namespace MicromegasTracing

#endif // UE_EXTERNAL_PROFILING_ENABLED
