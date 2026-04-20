#pragma once
//
//  MicromegasTracing/NetTraceWriter.h
//
#include "Async/Mutex.h"
#include "HAL/Platform.h"
#include "Logging/LogMacros.h"
#include "MicromegasTracing/Fwd.h"
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "Templates/SharedPointer.h"
#include "UObject/NameTypes.h"
#include <atomic>

// Diagnostic log category for net trace state events (re-entry, etc.). Kept in
// Core so the writer can log without depending on the sink module. Quiet on the
// hot path — only fires when nested connection scopes are detected.
DECLARE_LOG_CATEGORY_EXTERN(LogMicromegasNet, Log, All);

namespace MicromegasTracing
{
	enum class ENetTraceVerbosity : uint8
	{
		Off = 0,
		Packets = 1,
		RootObjects = 2,
		Objects = 3,
		Properties = 4,
	};

	class CORE_API NetTraceWriter
	{
	public:
		NetTraceWriter(const TSharedPtr<EventSink, ESPMode::ThreadSafe>& Sink, size_t BufferSize, ENetTraceVerbosity Verbosity);

		void InitStream(const FString& ProcessId, const FString& StreamId, const DualTime& StartTime);
		NetStreamPtr GetStream() const { return Stream; }

		void Flush();
		void SetVerbosity(ENetTraceVerbosity Level);
		ENetTraceVerbosity GetVerbosity() const;

		void Suspend();
		void Resume();
		void BeginConnection(FName ConnectionName, bool bIsOutgoing);
		void EndConnection();
		void BeginObject(StaticStringRef ObjectName);
		void EndObject(uint32 BitSize);
		void Property(StaticStringRef PropertyName, uint32 BitSize);
		void BeginRPC(StaticStringRef FunctionName);
		void EndRPC(uint32 BitSize);

	private:
		template <typename T>
		void QueueEvent(const T& Event);

		// Pushes `Event`, then either flushes (which unlocks) or unlocks.
		// Precondition: `Mutex` is held by the caller.
		template <typename T>
		void PushAndUnlock(const T& Event);

		void FlushImpl(UE::FMutex& Mutex);

		TSharedPtr<EventSink, ESPMode::ThreadSafe> Sink;
		UE::FMutex Mutex;
		NetStreamPtr Stream;
		size_t BufferSize;
		std::atomic<uint8> Verbosity;
		// Snapshotted from `Verbosity` at the outermost BeginConnection and used for
		// every gating decision within that scope (BeginObject / Property / BeginRPC /
		// EndRPC). This guarantees internally consistent output — CVar-driven
		// verbosity changes only take effect at the next connection scope boundary,
		// eliminating orphaned Begin/End pairs and partially-gated subtrees. Carries
		// over between scopes; the next BeginConnection overwrites it.
		ENetTraceVerbosity EffectiveVerbosity = ENetTraceVerbosity::Off;
		// Signed so unmatched Begin/End decrements surface as negative values instead of
		// silently wrapping uint8 to 255. Guarded by ensureMsgf on the decrement paths.
		int32 SuspendDepth = 0;
		int32 ConnectionDepth = 0; // connection scopes do not nest; only outermost emits
		int32 ObjectDepth = 0;
		int32 EmittedDepth = 0;
		uint32 AccumulatedBits = 0;
		// Captured at the outermost BeginConnection so a nested BeginConnection can
		// log both (name, direction) pairs. Direction is needed because re-entry
		// often happens on the same UNetConnection (e.g. Client RPC during PostLogin);
		// names match, only direction differs.
		FName OuterConnectionName;
		bool bOuterIsOutgoing = false;
		NetEventQueue::CheckPoint ConnectionScopeCheckPoint; // valid only inside an emitted connection scope
		NetEventQueue::CheckPoint RootScopeCheckPoint; // valid only inside an emitted root object scope
	};

} // namespace MicromegasTracing
