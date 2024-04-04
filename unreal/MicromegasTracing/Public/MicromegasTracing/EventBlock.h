#pragma once
//
//  MicromegasTracing/EventBlock.h
//
#include <string>
#include "MicromegasTracing/HeterogeneousQueue.h"
#include "MicromegasTracing/DualTime.h"

namespace MicromegasTracing
{
	template <typename QueueT>
	class EventBlock
	{
	public:
		typedef QueueT Queue;

		EventBlock(const std::wstring& streamId, const DualTime& begin, size_t bufferSize)
			: StreamId(streamId)
			, Begin(begin)
			, Events(bufferSize)
			, Capacity(bufferSize)
		{
		}

		void Close(const DualTime& end)
		{
			End = end;
		}

		const std::wstring& GetStreamId() const
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
			assert(End.Timestamp != 0);
			return End;
		}

	private:
		std::wstring StreamId;
		DualTime Begin;
		DualTime End;
		QueueT Events;
		size_t Capacity;
	};
} // namespace MicromegasTracing
