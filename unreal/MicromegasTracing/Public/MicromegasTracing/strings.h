#pragma once
//
//  MicromegasTracing/strings.h
//
#include <cassert> //todo: change to check
#include <vector> //todo: change to TArray
#include "Misc/CString.h"
#include "UObject/NameTypes.h"

struct FNameEntry;
class FName;

namespace MicromegasTracing
{
	template <typename T>
	struct GetEventMetadata;

	namespace StringCodec
	{
		enum Type : uint8
		{
			Ansi = 0,
			Wide = 1,
			Utf8 = 2,
			UnrealName = 3,
		};
	} // namespace StringCodec

	class StringReference
	{
	public:
		StringReference(const void* ptr, uint32 sizeBytes, StringCodec::Type codec)
			: Ptr(ptr)
			, SizeBytes(sizeBytes)
			, Codec(codec)
		{
		}

		StringCodec::Type GetCodec() const
		{
			return Codec;
		}

		uint32 GetSizeBytes() const
		{
			return SizeBytes;
		}

	protected:
		const void* Ptr;
		uint32 SizeBytes;
		StringCodec::Type Codec;
	};

	//
	// DynamicString points to a temporary buffer
	//   Serializing a DynamicString in queue copies the whole buffer
	//
	struct DynamicString : public StringReference
	{
		explicit DynamicString(const char* ptr)
			: StringReference(ptr, TCString<char>::Strlen(ptr), StringCodec::Ansi)
		{
		}

		explicit DynamicString(const WIDECHAR* ptr)
			: StringReference(ptr, TCString<WIDECHAR>::Strlen(ptr) * sizeof(WIDECHAR), StringCodec::Wide)
		{
		}

		explicit DynamicString(const StringReference& ref)
			: StringReference(ref)
		{
			assert(ref.GetCodec() == StringCodec::Ansi || ref.GetCodec() == StringCodec::Wide);
		}

		explicit DynamicString(const FString& str)
			: DynamicString(*str)
		{
		}

		const void* GetPtr() const
		{
			return Ptr;
		}
	};

	template <typename T>
	struct Serializer;

	namespace details
	{
		template <typename T>
		void WritePOD(const T& value, std::vector<uint8>& buffer);

		template <typename T>
		const T& ReadPOD(const std::vector<uint8>& buffer, size_t& cursor);
	} // namespace details

	template <>
	struct Serializer<DynamicString>
	{
		static uint32 GetHeaderSize()
		{
			return 1			  // codec
				+ sizeof(uint32); // size in bytes
		}

		static uint32 GetSize(const DynamicString& value)
		{
			return GetHeaderSize()
				+ value.GetSizeBytes(); // buffer
		}

		static void Write(const DynamicString& value, std::vector<uint8>& buffer)
		{
			assert(value.GetCodec() == StringCodec::Ansi || value.GetCodec() == StringCodec::Wide);
			details::WritePOD(value.GetCodec(), buffer);
			details::WritePOD(value.GetSizeBytes(), buffer);
			const uint8* beginString = reinterpret_cast<const uint8*>(value.GetPtr());
			buffer.insert(buffer.end(), beginString, beginString + value.GetSizeBytes());
		}

		template <typename Callback>
		static void Read(Callback callback, const std::vector<uint8>& buffer, size_t& cursor)
		{
			StringCodec::Type codec = details::ReadPOD<StringCodec::Type>(buffer, cursor);
			uint32 bufferSize = details::ReadPOD<uint32>(buffer, cursor);
			const void* ptr = &buffer[0] + cursor;
			cursor += bufferSize;

			DynamicString dynStr(StringReference(ptr, bufferSize, codec));
			callback(dynStr);
		}
	};

	struct StaticStringRef : public StringReference
	{
		explicit StaticStringRef(const ANSICHAR* ptr)
			: StringReference(ptr, TCString<ANSICHAR>::Strlen(ptr), StringCodec::Ansi)
		{
		}

		explicit StaticStringRef(const WIDECHAR* ptr)
			: StringReference(ptr, TCString<WIDECHAR>::Strlen(ptr) * sizeof(WIDECHAR), StringCodec::Wide)
		{
		}

		explicit StaticStringRef(const FNameEntry* ptr)
			: StringReference(ptr, 0, StringCodec::UnrealName)
		{
		}

		StaticStringRef(FName Name)
			: StaticStringRef(Name.GetDisplayNameEntry())
		{
		}

		uint64 GetID() const
		{
			return reinterpret_cast<uint64>(Ptr);
		}

		friend struct GetEventMetadata<StaticStringRef>;
	};
} // namespace MicromegasTracing
