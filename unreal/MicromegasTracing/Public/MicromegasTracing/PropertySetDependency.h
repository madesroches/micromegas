#pragma once
//
//  MicromegasTracing/PropertySetDependency.h
//
namespace MicromegasTracing
{
	struct Property
	{
	public:
		Property(const StaticStringRef& InKey, const StaticStringRef& InValue)
			: Name(InKey)
			, Value(InValue)
		{
		}

		// the field name need to match the rust version
		const StaticStringRef Name;
		const StaticStringRef Value;
	};

	template <>
	struct GetEventMetadata<Property>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("Property"),
				sizeof(Property),
				false,
				{ MAKE_UDT_MEMBER_METADATA(Property, "name", Name, StaticStringRef, true),
					MAKE_UDT_MEMBER_METADATA(Property, "value", Value, StaticStringRef, true) });
		}
	};

	struct PropertySetDependency
	{
	public:
		explicit PropertySetDependency(const PropertySet* Set)
			: Properties(Set)
		{
		}

		const PropertySet* Properties;
	};

	template <>
	struct Serializer<PropertySetDependency>
	{
		static bool IsSizeStatic()
		{
			return false;
		}

		static uint32 GetSize(const PropertySetDependency& dep)
		{
			uint32 HeaderSize = sizeof(uint64) // id
				+ sizeof(uint32);			   // number of properties;
			uint32 ContainerSize = dep.Properties->GetContext().Num() * sizeof(Property);
			return HeaderSize + ContainerSize;
		}

		static void Write(const PropertySetDependency& dep, std::vector<uint8>& buffer)
		{
			details::WritePOD(dep.Properties, buffer);
			uint32 NbProperties = dep.Properties->GetContext().Num();
			details::WritePOD(NbProperties, buffer);
			for (const TPair<FName, FName>& Prop : dep.Properties->GetContext())
			{
				details::WritePOD(Property(Prop.Key, Prop.Value), buffer);
			}
		}
	};

	template <>
	struct GetEventMetadata<PropertySetDependency>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(
				TEXT("PropertySetDependency"),
				0, // requires custom parsing logic
				false,
				{});
		}
	};

} // namespace MicromegasTracing
