#pragma once
//
//  MicromegasTracing/PropertySet.h
//
#include "Containers/Map.h"
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
	class Property
	{
	public:
		Property(const StaticStringRef& InKey, const StaticStringRef& InValue)
			: Key(InKey)
			, Value(InValue)
		{
		}

	private:
		const StaticStringRef Key;
		const StaticStringRef Value;
	};

	using TContext = TMap<FName, FName>;

	// PropertySet is an immutable and eternal set of key-value pairs
	class PropertySet
	{
	public:
		const TContext& GetContext() const
		{
			return Properties;
		}

	private:
		friend class PropertySetStore;
		explicit PropertySet(const TContext& Context);

		const TContext Properties;
	};

} // namespace MicromegasTracing
