#pragma once
//
//  MicromegasTracing/LogEvents.h
//
#include "Containers/Array.h"
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
	namespace LogLevel
	{
		enum Type : uint8
		{
			Invalid = 0,
			Fatal = 1,
			Error = 2,
			Warn = 3,
			Info = 4,
			Debug = 5,
			Trace = 6
		};
	} // namespace LogLevel

	struct TaggedLogInteropEvent
	{
		uint64 Timestamp;
		LogLevel::Type Level;
		StaticStringRef Target;
		const PropertySet* Properties;
		DynamicString Msg;

		TaggedLogInteropEvent(uint64 InTimestamp,
			LogLevel::Type InLevel,
			StaticStringRef InTarget,
			const PropertySet* InProperties,
			DynamicString InMsg)
			: Timestamp(InTimestamp)
			, Level(InLevel)
			, Target(InTarget)
			, Properties(InProperties)
			, Msg(InMsg)
		{
		}
	};

	template <>
	struct Serializer<TaggedLogInteropEvent>
	{
		typedef TaggedLogInteropEvent T;

		static bool IsSizeStatic()
		{
			return false;
		}

		static uint32 GetSize(const T& value)
		{
			return sizeof(uint64)		  // time
				+ 1						  // level
				+ sizeof(StaticStringRef) // target
				+ sizeof(uint64)		  // properties
				+ Serializer<DynamicString>::GetSize(value.Msg);
		}

		static void Write(const T& value, TArray<uint8>& buffer)
		{
			details::WritePOD(value.Timestamp, buffer);
			details::WritePOD(value.Level, buffer);
			details::WritePOD(value.Target, buffer);
			details::WritePOD(value.Properties, buffer);
			Serializer<DynamicString>::Write(value.Msg, buffer);
		}

		template <typename Callback>
		static void Read(Callback& callback, const TArray<uint8>& buffer, size_t& cursor)
		{
			uint64 Timestamp = details::ReadPOD<uint64>(buffer, cursor);
			LogLevel::Type Level = details::ReadPOD<LogLevel::Type>(buffer, cursor);
			StaticStringRef Target = details::ReadPOD<StaticStringRef>(buffer, cursor);
			const PropertySet* Properties = details::ReadPOD<PropertySet*>(buffer, cursor);
			Serializer<DynamicString>::Read([&](const DynamicString& Msg) {
				TaggedLogInteropEvent Event(Timestamp, Level, Target, Properties, Msg);
				callback(Event);
			},
				buffer, cursor);
		}
	};

	struct LogMetadata
	{
		const TCHAR* Target;
		const TCHAR* Msg;
		const char* File;
		uint32 Line;
		LogLevel::Type Level;

		LogMetadata(LogLevel::Type level,
			const TCHAR* target,
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
		const StaticStringRef Target;
		const StaticStringRef Msg;
		const StaticStringRef File;
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

	struct TaggedLogString
	{
		const LogMetadata* Desc;
		const PropertySet* Properties;
		uint64 Timestamp;
		DynamicString Msg;

		TaggedLogString(
			const LogMetadata* InDesc,
			const PropertySet* InProperties,
			uint64 InTimestamp,
			DynamicString InMsg)
			: Desc(InDesc)
			, Properties(InProperties)
			, Timestamp(InTimestamp)
			, Msg(InMsg)
		{
		}
	};

	template <>
	struct Serializer<TaggedLogString>
	{
		typedef TaggedLogString T;

		static bool IsSizeStatic()
		{
			return false;
		}

		static uint32 GetSize(const T& value)
		{
			return sizeof(uint64) // desc ptr
				+ sizeof(uint64)  // properties ptr
				+ sizeof(int64)	  // time
				+ Serializer<DynamicString>::GetSize(value.Msg);
		}

		static void Write(const T& value, TArray<uint8>& buffer)
		{
			details::WritePOD(value.Desc, buffer);
			details::WritePOD(value.Properties, buffer);
			details::WritePOD(value.Timestamp, buffer);
			Serializer<DynamicString>::Write(value.Msg, buffer);
		}

		template <typename Callback>
		static void Read(Callback& callback, const TArray<uint8>& buffer, size_t& cursor)
		{
			const LogMetadata* Desc = details::ReadPOD<LogMetadata*>(buffer, cursor);
			const PropertySet* Properties = details::ReadPOD<PropertySet*>(buffer, cursor);
			uint64 timestamp = details::ReadPOD<uint64>(buffer, cursor);
			Serializer<DynamicString>::Read([&](const DynamicString& msg) {
				TaggedLogString event(Desc, Properties, timestamp, msg);
				callback(event);
			},
				buffer, cursor);
		}
	};

} // namespace MicromegasTracing
