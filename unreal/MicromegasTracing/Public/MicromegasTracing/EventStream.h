#pragma once
//
//  MicromegasTracing/EventStream.h
//
#include <vector>
#include <map>

namespace MicromegasTracing
{
	template <typename EventBlockT, size_t BUFFER_PADDING>
	class EventStreamImpl
	{
	public:
		typedef EventBlockT EventBlock;
		typedef std::shared_ptr<EventBlockT> BlockPtr;

		EventStreamImpl(const FString& processId,
			const FString& streamId,
			const BlockPtr& block,
			const std::vector<FString>& tags)
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

		const std::vector<FString>& GetTags() const
		{
			return Tags;
		}

		const std::map<FString, FString>& GetProperties() const
		{
			return Properties;
		}

		void SetProperty(const FString& name, const FString& value)
		{
			Properties[name] = value;
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
		std::vector<FString> Tags;
		std::map<FString, FString> Properties;
	};

} // namespace MicromegasTracing
