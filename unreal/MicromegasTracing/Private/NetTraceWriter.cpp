//
//  MicromegasTracing/NetTraceWriter.cpp
//
#include "MicromegasTracing/NetTraceWriter.h"
#include "HAL/PlatformTime.h"
#include "MicromegasTracing/DualTime.h"
#include "MicromegasTracing/EventSink.h"
#include "MicromegasTracing/EventStream.h"
#include "MicromegasTracing/Macros.h"
#include "MicromegasTracing/NetBlock.h"
#include "MicromegasTracing/NetEvents.h"

DEFINE_LOG_CATEGORY(LogMicromegasNet);

namespace MicromegasTracing
{
	NetTraceWriter::NetTraceWriter(const TSharedPtr<EventSink, ESPMode::ThreadSafe>& InSink, size_t InBufferSize, ENetTraceVerbosity InVerbosity)
		: Sink(InSink)
		, BufferSize(InBufferSize)
		, Verbosity(static_cast<uint8>(InVerbosity))
	{
	}

	void NetTraceWriter::InitStream(const FString& ProcessId, const FString& StreamId, const DualTime& StartTime)
	{
		NetBlockPtr NewBlock = MakeShared<NetBlock>(StreamId, StartTime, BufferSize, 0);
		Stream = MakeShared<NetStream>(ProcessId, StreamId, NewBlock, TArray<FString>({ TEXT("net") }));
	}

	void NetTraceWriter::FlushImpl(UE::FMutex& InMutex)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (Stream->GetCurrentBlock().IsEmpty())
		{
			InMutex.Unlock();
			return;
		}
		DualTime Now = DualTime::Now();
		size_t NewOffset = Stream->GetCurrentBlock().GetOffset() + Stream->GetCurrentBlock().GetEvents().GetNbEvents();
		NetBlockPtr NewBlock = MakeShared<NetBlock>(Stream->GetStreamId(),
			Now,
			BufferSize,
			NewOffset);
		NetBlockPtr FullBlock = Stream->SwapBlocks(NewBlock);
		FullBlock->Close(Now);
		InMutex.Unlock();
		Sink->OnProcessNetBlock(FullBlock);
	}

	template <typename T>
	void NetTraceWriter::PushAndUnlock(const T& Event)
	{
		Stream->GetCurrentBlock().GetEvents().Push(Event);
		if (Stream->IsFull())
		{
			FlushImpl(Mutex); // unlocks the mutex
		}
		else
		{
			Mutex.Unlock();
		}
	}

	template <typename T>
	void NetTraceWriter::QueueEvent(const T& Event)
	{
		Mutex.Lock();
		PushAndUnlock(Event);
	}

	void NetTraceWriter::Flush()
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		Mutex.Lock();
		FlushImpl(Mutex); // unlocks the mutex
	}

	void NetTraceWriter::SetVerbosity(ENetTraceVerbosity Level)
	{
		Verbosity.store(static_cast<uint8>(Level), std::memory_order_relaxed);
	}

	ENetTraceVerbosity NetTraceWriter::GetVerbosity() const
	{
		return static_cast<ENetTraceVerbosity>(Verbosity.load(std::memory_order_relaxed));
	}

	void NetTraceWriter::Suspend()
	{
		++SuspendDepth;
	}

	void NetTraceWriter::Resume()
	{
		if (!ensureMsgf(SuspendDepth > 0, TEXT("MicromegasTracing: NetTraceWriter::Resume() without matching Suspend")))
		{
			return;
		}
		--SuspendDepth;
	}

	void NetTraceWriter::BeginConnection(FName ConnectionName, bool bIsOutgoing)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (SuspendDepth > 0) return;

		// Connection scopes do not nest: only the outermost emits, nested calls are
		// absorbed so an inner scope can't destroy the outer's AccumulatedBits.
		// Re-entry happens on known-normal UE paths (e.g. Client RPC during PostLogin
		// synchronously opens an outgoing scope inside an incoming packet handler),
		// so we surface it with a log line — counting the inner bits is unreliable
		// (they either roll into the outer's AccumulatedBits or are dropped entirely
		// if they close while ObjectDepth > 0).
		if (ConnectionDepth++ > 0)
		{
			UE_LOG(LogMicromegasNet, Log,
				TEXT("Nested net scope: '%s' (%s) inside outer '%s' (%s) — bits absorbed or dropped"),
				*ConnectionName.ToString(), bIsOutgoing ? TEXT("outgoing") : TEXT("incoming"),
				*OuterConnectionName.ToString(), bOuterIsOutgoing ? TEXT("outgoing") : TEXT("incoming"));
			return;
		}

		// Reset depth / accumulator at the outermost scope boundary (safety net
		// for any imbalance carried over from the previous scope).
		ObjectDepth = 0;
		EmittedDepth = 0;
		AccumulatedBits = 0;
		// Capture for the nested-scope log line above.
		OuterConnectionName = ConnectionName;
		bOuterIsOutgoing = bIsOutgoing;

		// Snapshot verbosity for the lifetime of this scope. All gating decisions
		// (BeginObject / Property / BeginRPC / EndRPC) read EffectiveVerbosity instead
		// of the atomic, so a CVar-driven change mid-scope cannot produce orphan
		// events (e.g. a nested BeginObject emitted without a matching root Begin
		// that was suppressed when the scope started). Changes take effect at the
		// next outermost BeginConnection.
		EffectiveVerbosity = static_cast<ENetTraceVerbosity>(Verbosity.load(std::memory_order_relaxed));

		if (EffectiveVerbosity >= ENetTraceVerbosity::Packets)
		{
			// Snapshot before pushing so EndConnection can rewind the entire scope
			// when AccumulatedBits == 0 (nothing was sent). Same pattern as root
			// object elision.
			Mutex.Lock();
			ConnectionScopeCheckPoint = Stream->GetCurrentBlock().GetEvents().Snapshot();
			PushAndUnlock(NetConnectionBeginEvent(FPlatformTime::Cycles64(), StaticStringRef(ConnectionName), bIsOutgoing ? 1 : 0));
		}
	}

	void NetTraceWriter::EndConnection()
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (SuspendDepth > 0) return;

		// Pair with the outermost Begin. Nested Ends are no-ops.
		if (ConnectionDepth == 0) return; // stray End without matching Begin — ignore
		if (--ConnectionDepth > 0) return;

		if (EffectiveVerbosity >= ENetTraceVerbosity::Packets)
		{
			// Elide the entire connection scope if nothing was sent.
			Mutex.Lock();
			auto& Events = Stream->GetCurrentBlock().GetEvents();
			const bool bCanRewind =
				   AccumulatedBits == 0
				&& ConnectionScopeCheckPoint.IsValidFor(Events);

			if (bCanRewind)
			{
				Events.RewindTo(ConnectionScopeCheckPoint);
				Mutex.Unlock();
			}
			else
			{
				PushAndUnlock(NetConnectionEndEvent(FPlatformTime::Cycles64(), AccumulatedBits));
			}
		}
		AccumulatedBits = 0;
	}

	void NetTraceWriter::BeginObject(StaticStringRef ObjectName)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (SuspendDepth > 0) return;

		const bool bIsRoot = (ObjectDepth == 0);
		ENetTraceVerbosity Required = bIsRoot
			? ENetTraceVerbosity::RootObjects
			: ENetTraceVerbosity::Objects;
		++ObjectDepth;

		if (EffectiveVerbosity >= Required)
		{
			++EmittedDepth;
			if (bIsRoot)
			{
				// Snapshot before pushing so EndObject can rewind the entire subtree
				// when BitSize == 0 (nothing reached the wire). Mutex held across
				// snapshot+push to prevent a flush from invalidating the checkpoint
				// between the two operations.
				Mutex.Lock();
				RootScopeCheckPoint = Stream->GetCurrentBlock().GetEvents().Snapshot();
				PushAndUnlock(NetObjectBeginEvent(FPlatformTime::Cycles64(), ObjectName));
			}
			else
			{
				QueueEvent(NetObjectBeginEvent(FPlatformTime::Cycles64(), ObjectName));
			}
		}
	}

	void NetTraceWriter::EndObject(uint32 BitSize)
	{
		MICROMEGAS_SPAN_FUNCTION("MicromegasTracing");
		if (SuspendDepth > 0) return;

		if (!ensureMsgf(ObjectDepth > 0, TEXT("MicromegasTracing: NetTraceWriter::EndObject() without matching BeginObject")))
		{
			return;
		}
		--ObjectDepth;

		if (EmittedDepth > 0 && EmittedDepth > ObjectDepth)
		{
			--EmittedDepth;

			if (ObjectDepth == 0)
			{
				// Root scope closing — elide the entire subtree (BeginObject +
				// all nested events) if BitSize == 0, meaning nothing reached
				// the wire. Mutex held so a flush can't swap the block mid-check.
				Mutex.Lock();
				auto& Events = Stream->GetCurrentBlock().GetEvents();
				const bool bCanRewind =
					   BitSize == 0
					&& RootScopeCheckPoint.IsValidFor(Events);

				if (bCanRewind)
				{
					Events.RewindTo(RootScopeCheckPoint);
					Mutex.Unlock();
				}
				else
				{
					PushAndUnlock(NetObjectEndEvent(FPlatformTime::Cycles64(), BitSize));
				}
			}
			else
			{
				QueueEvent(NetObjectEndEvent(FPlatformTime::Cycles64(), BitSize));
			}
		}

		if (ObjectDepth == 0)
		{
			AccumulatedBits += BitSize;
		}
	}

	void NetTraceWriter::Property(StaticStringRef PropertyName, uint32 BitSize)
	{
		if (SuspendDepth > 0) return;

		if (EffectiveVerbosity >= ENetTraceVerbosity::Properties)
		{
			QueueEvent(NetPropertyEvent(FPlatformTime::Cycles64(), PropertyName, BitSize));
		}
	}

	void NetTraceWriter::BeginRPC(StaticStringRef FunctionName)
	{
		if (SuspendDepth > 0) return;

		if (EffectiveVerbosity >= ENetTraceVerbosity::Properties)
		{
			QueueEvent(NetRPCBeginEvent(FPlatformTime::Cycles64(), FunctionName));
		}
	}

	void NetTraceWriter::EndRPC(uint32 BitSize)
	{
		if (SuspendDepth > 0) return;

		if (EffectiveVerbosity >= ENetTraceVerbosity::Properties)
		{
			QueueEvent(NetRPCEndEvent(FPlatformTime::Cycles64(), BitSize));
		}

		if (ObjectDepth == 0)
		{
			AccumulatedBits += BitSize;
		}
	}

} // namespace MicromegasTracing
