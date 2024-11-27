#pragma once
//
//  MicromegasTracing/PropertySetStore.h
//
#include "Containers/Map.h"
#include "HAL/CriticalSection.h"
#include "UObject/NameTypes.h"

namespace MicromegasTracing
{
	class PropertySet;

	using TContext = TMap<FName, FName>;

	struct SetStoreKeyFuncs : BaseKeyFuncs<PropertySet*, const TContext*>
	{
		static const TContext* GetSetKey(PropertySet* set);
		static bool Matches(const TContext* Lhs, const TContext* Rhs);
		static uint32 GetKeyHash(const TContext* Key);
	};

	class CORE_API PropertySetStore
	{
	public:
		PropertySetStore();
		~PropertySetStore();

		PropertySet* Get(const TContext& Context);

	private:
		FCriticalSection Mutex;
		TSet<PropertySet*, SetStoreKeyFuncs> PropertySets;
	};

} // namespace MicromegasTracing
