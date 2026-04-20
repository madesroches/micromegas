#pragma once
//
//  MicromegasTracing/NetMetadata.h
//
#include "MicromegasTracing/NetEvents.h"
#include "MicromegasTracing/QueueMetadata.h"

namespace MicromegasTracing
{
	template <>
	struct GetEventMetadata<NetConnectionBeginEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetConnectionBeginEvent"),
				sizeof(NetConnectionBeginEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetConnectionBeginEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetConnectionBeginEvent, "connection_name", ConnectionName, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(NetConnectionBeginEvent, "is_outgoing", bIsOutgoing, uint8, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetConnectionEndEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetConnectionEndEvent"),
				sizeof(NetConnectionEndEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetConnectionEndEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetConnectionEndEvent, "bit_size", BitSize, uint32, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetObjectBeginEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetObjectBeginEvent"),
				sizeof(NetObjectBeginEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetObjectBeginEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetObjectBeginEvent, "object_name", ObjectName, StaticStringRef, true),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetObjectEndEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetObjectEndEvent"),
				sizeof(NetObjectEndEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetObjectEndEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetObjectEndEvent, "bit_size", BitSize, uint32, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetPropertyEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetPropertyEvent"),
				sizeof(NetPropertyEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetPropertyEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetPropertyEvent, "property_name", PropertyName, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(NetPropertyEvent, "bit_size", BitSize, uint32, false),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetRPCBeginEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetRPCBeginEvent"),
				sizeof(NetRPCBeginEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetRPCBeginEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetRPCBeginEvent, "function_name", FunctionName, StaticStringRef, true),
				});
		}
	};

	template <>
	struct GetEventMetadata<NetRPCEndEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("NetRPCEndEvent"),
				sizeof(NetRPCEndEvent),
				false,
				{
					MAKE_UDT_MEMBER_METADATA(NetRPCEndEvent, "time", Timestamp, uint64, false),
					MAKE_UDT_MEMBER_METADATA(NetRPCEndEvent, "bit_size", BitSize, uint32, false),
				});
		}
	};
} // namespace MicromegasTracing
