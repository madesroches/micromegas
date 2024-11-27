#pragma once
//
//  MicromegasTracing/EventBlock.h
//
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/DualTime.h"

namespace MicromegasTracing
{
	template <typename QueueT>
	class EventBlock
	{
	public:
		typedef QueueT Queue;

		EventBlock(const FString& streamId, const DualTime& begin, size_t bufferSize, size_t offset)
			: StreamId(streamId)
			, Begin(begin)
			, Events(bufferSize)
			, Capacity(bufferSize)
			, ObjectOffset(offset)
		{
		}

		void Close(const DualTime& end)
		{
			End = end;
		}

		const FString& GetStreamId() const
		{
			return StreamId;
		}

		QueueT& GetEvents()
		{
			return Events;
		}

		const QueueT& GetEvents() const
		{
			return Events;
		}

		size_t GetCapacity() const
		{
			return Capacity;
		}

		size_t GetSizeBytes() const
		{
			return Events.GetSizeBytes();
		}

		const DualTime& GetBeginTime() const
		{
			return Begin;
		}

		const DualTime& GetEndTime() const
		{
			check(End.Timestamp != 0);
			return End;
		}

		size_t GetOffset()const
		{
			return ObjectOffset;
		}

		bool IsEmpty()const
		{
			return Events.GetNbEvents() == 0;
		}

	private:
		FString StreamId;
		DualTime Begin;
		DualTime End;
		QueueT Events;
		size_t Capacity;
		size_t ObjectOffset;
	};
} // namespace MicromegasTracing
