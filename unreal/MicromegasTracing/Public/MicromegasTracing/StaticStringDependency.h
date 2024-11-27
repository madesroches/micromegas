#pragma once
//
//  MicromegasTracing/StaticStringDependency.h
//
#include "Containers/Array.h"
#include "MicromegasTracing/QueueMetadata.h"
#include "UObject/NameTypes.h"

namespace MicromegasTracing
{
	struct StaticStringDependency
	{
		StaticStringRef Ref;
		explicit StaticStringDependency(const StaticStringRef& ref)
			: Ref(ref)
		{
		}
	};

	template <>
	struct Serializer<StaticStringDependency>
	{
		static bool IsSizeStatic()
		{
			return false;
		}

		static uint32 GetSize(const StaticStringDependency& dep)
		{
			size_t bytes = 0;
			if (dep.Ref.GetCodec() == StringCodec::UnrealName)
			{
				const FNameEntry* entry = reinterpret_cast<const FNameEntry*>(dep.Ref.GetID());
				if (entry->IsWide())
				{
					bytes = entry->GetNameLength() * 2;
				}
				else
				{
					bytes = entry->GetNameLength();
				}
			}
			else
			{
				bytes = dep.Ref.GetSizeBytes();
			}
			return sizeof(uint64) + Serializer<DynamicString>::GetHeaderSize() + bytes;
		}

		static void Write(const StaticStringDependency& dep, TArray<uint8>& buffer)
		{
			details::WritePOD(dep.Ref.GetID(), buffer);
			if (dep.Ref.GetCodec() == StringCodec::UnrealName)
			{
				// we should not copy the string on the stack, we should copy it directly in the buffer
				union
				{
					ANSICHAR AnsiName[NAME_SIZE];
					WIDECHAR WideName[NAME_SIZE];
				};
				const FNameEntry* entry = reinterpret_cast<const FNameEntry*>(dep.Ref.GetID());
				if (entry->IsWide())
				{
					size_t bytes = entry->GetNameLength() * 2;
					entry->GetWideName(WideName);
					DynamicString str(StringReference(WideName, bytes, StringCodec::Wide));
					Serializer<DynamicString>::Write(str, buffer);
				}
				else
				{
					size_t bytes = entry->GetNameLength();
					entry->GetAnsiName(AnsiName);
					DynamicString str(StringReference(AnsiName, bytes, StringCodec::Ansi));
					Serializer<DynamicString>::Write(str, buffer);
				}
			}
			else
			{
				DynamicString str(dep.Ref);
				Serializer<DynamicString>::Write(str, buffer);
			}
		}
	};

	template <>
	struct GetEventMetadata<StaticStringDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("StaticStringDependency"),
				0, // requires custom parsing logic
				false,
				{});
		}
	};
} // namespace MicromegasTracing
