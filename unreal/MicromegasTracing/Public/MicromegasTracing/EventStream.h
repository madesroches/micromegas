#pragma once
//
//  MicromegasTracing/EventStream.h
//
#include "Containers/Array.h"
#include "Containers/Map.h"
#include "Templates/SharedPointer.h"

namespace MicromegasTracing
{
	template <typename EventBlockT, size_t BUFFER_PADDING>
	class EventStreamImpl
	{
	public:
		typedef EventBlockT EventBlock;
		typedef TSharedPtr<EventBlockT, ESPMode::ThreadSafe> BlockPtr;

		EventStreamImpl(const FString& processId,
			const FString& streamId,
			const BlockPtr& block,
			const TArray<FString>& tags)
			: ProcessId(processId)
			, StreamId(streamId)
			, Tags(tags)
		{
			assert(block->GetCapacity() > BUFFER_PADDING);
			FullThreshold = block->GetCapacity() - BUFFER_PADDING;
			CurrentBlock = block;
		}

		const FString& GetProcessId() const
		{
			return ProcessId;
		}

		const FString& GetStreamId() const
		{
			return StreamId;
		}

		const TArray<FString>& GetTags() const
		{
			return Tags;
		}

		const TMap<FString, FString>& GetProperties() const
		{
			return Properties;
		}

		void SetProperty(const FString& name, const FString& value)
		{
			Properties.Add(name, value);
		}

		void MarkFull()
		{
			FullThreshold = 0;
		}

		BlockPtr SwapBlocks(const BlockPtr& newBlock)
		{
			BlockPtr old = CurrentBlock;
			CurrentBlock = newBlock;
			assert(CurrentBlock->GetCapacity() > BUFFER_PADDING);
			FullThreshold = CurrentBlock->GetCapacity() - BUFFER_PADDING;
			return old;
		}

		EventBlockT& GetCurrentBlock()
		{
			return *CurrentBlock;
		}

		const EventBlockT& GetCurrentBlock() const
		{
			return *CurrentBlock;
		}

		bool IsFull() const
		{
			return CurrentBlock->GetSizeBytes() >= FullThreshold;
		}

	private:
		FString ProcessId;
		FString StreamId;
		BlockPtr CurrentBlock;
		size_t FullThreshold;
		TArray<FString> Tags;
		TMap<FString, FString> Properties;
	};

} // namespace MicromegasTracing
