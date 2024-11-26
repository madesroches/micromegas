//
//  MicromegasTracing/DefaultContext.h
//

#pragma once
#include "Containers/Map.h"
#include "HAL/CriticalSection.h"
#include "MicromegasTracing/PropertySet.h"

namespace MicromegasTracing
{
	class PropertySetStore;
	
	// There should be only one instance of this class. It allows different subsystems to set properties in the global context.
	// The resulting propertyset is tagged to measures and log entries.
	class CORE_API DefaultContext
	{
	public:
		explicit DefaultContext(PropertySetStore* InStore);
		~DefaultContext();

		// Set, Unset and Clear are expensive and are not expected to be called frequently.
		// Since the keys and values are never freed, local cardinality has to be limited.
		void Set(FName Key, FName Value);
		void Unset(FName Key);
		void Clear();

		PropertySet* GetCurrentPropertySet()
		{
			return CurrentPropertySet;
		}

	private:
		void UpdatePropertySet();

		PropertySetStore* Store;
		FCriticalSection Mutex;
		TMap<FName, FName> Context;
		PropertySet* CurrentPropertySet = nullptr;
	};

} // namespace MicromegasTracing
