#pragma once
//
//  MicromegasTracing/ImageEvents.h
//
#include "Containers/Array.h"
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
	struct ImageEvent
	{
		uint64 Timestamp;
		DynamicString Name;
		DynamicString Format;
		DynamicBlob Data;

		ImageEvent(uint64 InTimestamp, DynamicString InName, DynamicString InFormat, DynamicBlob InData)
			: Timestamp(InTimestamp)
			, Name(InName)
			, Format(InFormat)
			, Data(InData)
		{
		}
	};

	template <>
	struct Serializer<ImageEvent>
	{
		static bool IsSizeStatic() { return false; }

		static uint32 GetSize(const ImageEvent& v)
		{
			return sizeof(uint64)
				+ Serializer<DynamicString>::GetSize(v.Name)
				+ Serializer<DynamicString>::GetSize(v.Format)
				+ Serializer<DynamicBlob>::GetSize(v.Data);
		}

		static void Write(const ImageEvent& v, TArray<uint8>& buffer)
		{
			details::WritePOD(v.Timestamp, buffer);
			Serializer<DynamicString>::Write(v.Name, buffer);
			Serializer<DynamicString>::Write(v.Format, buffer);
			Serializer<DynamicBlob>::Write(v.Data, buffer);
		}

		template <typename Callback>
		static void Read(Callback& callback, const TArray<uint8>& buffer, size_t& cursor)
		{
			uint64 Timestamp = details::ReadPOD<uint64>(buffer, cursor);
			Serializer<DynamicString>::Read([&](const DynamicString& Name) {
				Serializer<DynamicString>::Read([&](const DynamicString& Format) {
					Serializer<DynamicBlob>::Read([&](const DynamicBlob& Data) {
						ImageEvent Event(Timestamp, Name, Format, Data);
						callback(Event);
					},
						buffer, cursor);
				},
					buffer, cursor);
			},
				buffer, cursor);
		}
	};

} // namespace MicromegasTracing
