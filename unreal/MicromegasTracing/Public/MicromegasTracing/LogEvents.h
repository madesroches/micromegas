#pragma once
//
//  MicromegasTracing/LogEvents.h
//
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
	namespace LogLevel
	{
		enum Type : uint8
		{
			Invalid = 0,
			Error = 1,
			Warn = 2,
			Info = 3,
			Debug = 4,
			Trace = 5
		};
	} // namespace LogLevel

	struct LogStringInteropEvent
	{
		uint64 Timestamp;
		LogLevel::Type Level;
		StaticStringRef Target;
		DynamicString Msg;

		LogStringInteropEvent(uint64 timestamp,
			LogLevel::Type level,
			StaticStringRef target,
			DynamicString msg)
			: Timestamp(timestamp)
			, Level(level)
			, Target(target)
			, Msg(msg)
		{
		}
	};

	template <>
	struct Serializer<LogStringInteropEvent>
	{
		typedef LogStringInteropEvent T;

		static bool IsSizeStatic()
		{
			return false;
		}

		static uint32 GetSize(const T& value)
		{
			return sizeof(uint64) // time
				+ 1				  // level
				+ sizeof(StaticStringRef)
				+ Serializer<DynamicString>::GetSize(value.Msg);
		}

		static void Write(const T& value, std::vector<uint8>& buffer)
		{
			details::WritePOD(value.Timestamp, buffer);
			details::WritePOD(value.Level, buffer);
			details::WritePOD(value.Target, buffer);
			Serializer<DynamicString>::Write(value.Msg, buffer);
		}

		template <typename Callback>
		static void Read(Callback& callback, const std::vector<uint8>& buffer, size_t& cursor)
		{
			uint64 timestamp = details::ReadPOD<uint64>(buffer, cursor);
			LogLevel::Type level = details::ReadPOD<LogLevel::Type>(buffer, cursor);
			StaticStringRef target = details::ReadPOD<StaticStringRef>(buffer, cursor);
			Serializer<DynamicString>::Read([&](const DynamicString& msg) {
				LogStringInteropEvent event(timestamp, level, target, msg);
				callback(event);
			},
				buffer, cursor);
		}
	};

	struct LogMetadata
	{
		const char* Target;
		const TCHAR* Msg;
		const char* File;
		uint32 Line;
		LogLevel::Type Level;

		LogMetadata(LogLevel::Type level,
			const char* target,
			const TCHAR* msg,
			const char* file,
			uint32 line)
			: Target(target)
			, Msg(msg)
			, File(file)
			, Line(line)
			, Level(level)

		{
		}
	};

	struct LogMetadataDependency
	{
		uint64 Id;
		const char* Target;
		const TCHAR* Msg;
		const char* File;
		uint32 Line;
		LogLevel::Type Level;

		explicit LogMetadataDependency(const LogMetadata* logDesc)
			: Id(reinterpret_cast<uint64>(logDesc))
			, Target(logDesc->Target)
			, Msg(logDesc->Msg)
			, File(logDesc->File)
			, Line(logDesc->Line)
			, Level(logDesc->Level)
		{
		}
	};

	struct LogStaticStrEvent
	{
		const LogMetadata* Desc;
		uint64 Timestamp;

		LogStaticStrEvent(const LogMetadata* desc, uint64 timestamp)
			: Desc(desc)
			, Timestamp(timestamp)
		{
		}
	};

} // namespace MicromegasTracing
