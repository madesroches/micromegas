#include "SystemErrorReporter.h"
#include "HAL/PlatformStackWalk.h"
#include "HAL/PlatformMallocCrash.h"
#include "HttpManager.h"
#include "HttpModule.h"
#include "MicromegasTracing/Macros.h"
#include "Misc/CoreDelegates.h"

FSystemErrorReporter::FSystemErrorReporter()
{
	FCoreDelegates::OnHandleSystemError.AddRaw(this, &FSystemErrorReporter::OnSystemError);
}

FSystemErrorReporter::~FSystemErrorReporter()
{
	FCoreDelegates::OnHandleSystemError.RemoveAll(this);
}

void FSystemErrorReporter::OnSystemError()
{
	if (FPlatformMallocCrash::IsActive())
	{
		// For now if the PlatformCrash is active, we don't flush because it will cause problems.
		// 1. Cannot allocate memory (2MB limit on pre-allocated mem)
		// 2. Deadlocks if using `FGenericPlatformMallocCrash`.
		//   - FGenericPlatformMallocCrash will spin any non-crashing threads forever on Malloc calls
		//     This causes deadlocks with Micromegas sink threads on flush 
		//     -- LogMutex acquired in sink thread (which spins forever on allocation) and deadlocks on FlushLogStream which tries to acquire the lock.
		return;
	}
	
	const size_t MessageMaxSize = 65535;
	ANSICHAR* Message = (ANSICHAR*)FMemory::SystemMalloc(MessageMaxSize);
	if (Message == nullptr)
	{
		return;
	}
	Message[0] = 0;

	constexpr int MAX_DEPTH = 64;
	constexpr uint32 CALLS_TO_SKIP = 3; // let's remove part of the error handling code in the reported call stack
	uint64 BackTrace[MAX_DEPTH] = { 0 };
	uint32 BackTraceDepth = FPlatformStackWalk::CaptureStackBackTrace(BackTrace, MAX_DEPTH);
	for (uint32 StackDepth = CALLS_TO_SKIP; StackDepth < BackTraceDepth && BackTrace[StackDepth]; ++StackDepth)
	{
		FProgramCounterSymbolInfo SymbolInfo;
		FPlatformStackWalk::ProgramCounterToSymbolInfo(BackTrace[StackDepth], SymbolInfo);
		FPlatformStackWalk::SymbolInfoToHumanReadableString(SymbolInfo, Message, MessageMaxSize);
		FCStringAnsi::StrncatTruncateDest(Message, MessageMaxSize, LINE_TERMINATOR_ANSI);
	}
	MICROMEGAS_LOG("MicromegasTelemetrySink", MicromegasTracing::LogLevel::Fatal, Message);
	FMemory::SystemFree(Message);
	MicromegasTracing::Dispatch::FlushLogStream();
	MicromegasTracing::Dispatch::Shutdown();
	FHttpModule::Get().GetHttpManager().Flush(EHttpFlushReason::Default);
}
