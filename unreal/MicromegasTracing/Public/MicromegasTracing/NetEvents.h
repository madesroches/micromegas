#pragma once
//
//  MicromegasTracing/NetEvents.h
//
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
	struct NetConnectionBeginEvent
	{
		uint64 Timestamp;
		StaticStringRef ConnectionName;
		uint8 bIsOutgoing;

		NetConnectionBeginEvent(uint64 InTimestamp, const StaticStringRef& InConnectionName, uint8 InIsOutgoing)
			: Timestamp(InTimestamp)
			, ConnectionName(InConnectionName)
			, bIsOutgoing(InIsOutgoing)
		{
		}
	};

	struct NetConnectionEndEvent
	{
		uint64 Timestamp;
		uint32 BitSize;

		NetConnectionEndEvent(uint64 InTimestamp, uint32 InBitSize)
			: Timestamp(InTimestamp)
			, BitSize(InBitSize)
		{
		}
	};

	struct NetObjectBeginEvent
	{
		uint64 Timestamp;
		StaticStringRef ObjectName;

		NetObjectBeginEvent(uint64 InTimestamp, const StaticStringRef& InObjectName)
			: Timestamp(InTimestamp)
			, ObjectName(InObjectName)
		{
		}
	};

	struct NetObjectEndEvent
	{
		uint64 Timestamp;
		uint32 BitSize;

		NetObjectEndEvent(uint64 InTimestamp, uint32 InBitSize)
			: Timestamp(InTimestamp)
			, BitSize(InBitSize)
		{
		}
	};

	struct NetPropertyEvent
	{
		uint64 Timestamp;
		StaticStringRef PropertyName;
		uint32 BitSize;

		NetPropertyEvent(uint64 InTimestamp, const StaticStringRef& InPropertyName, uint32 InBitSize)
			: Timestamp(InTimestamp)
			, PropertyName(InPropertyName)
			, BitSize(InBitSize)
		{
		}
	};

	struct NetRPCBeginEvent
	{
		uint64 Timestamp;
		StaticStringRef FunctionName;

		NetRPCBeginEvent(uint64 InTimestamp, const StaticStringRef& InFunctionName)
			: Timestamp(InTimestamp)
			, FunctionName(InFunctionName)
		{
		}
	};

	struct NetRPCEndEvent
	{
		uint64 Timestamp;
		uint32 BitSize;

		NetRPCEndEvent(uint64 InTimestamp, uint32 InBitSize)
			: Timestamp(InTimestamp)
			, BitSize(InBitSize)
		{
		}
	};
} // namespace MicromegasTracing
