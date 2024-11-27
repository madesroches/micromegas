//
//  MicromegasTracing/PropertySetStore.cpp
//
#include "MicromegasTracing/PropertySetStore.h"
#include "MicromegasTracing/PropertySet.h"
#include "Misc/ScopeLock.h"

namespace MicromegasTracing
{
	namespace
	{
		uint32 HashProperties(const TContext& Context)
		{
			uint32 hash = 0;
			for (const TPair<FName, FName>& prop : Context)
			{
				hash = HashCombine(GetTypeHash(prop.Key), hash);
				hash = HashCombine(GetTypeHash(prop.Value), hash);
			}
			return hash;
		}
	} // namespace

	//
	// SetStoreKeyFuncs
	//
	const TContext* SetStoreKeyFuncs::GetSetKey(PropertySet* set)
	{
		return &set->GetContext();
	}
	bool SetStoreKeyFuncs::Matches(const TContext* Lhs, const TContext* Rhs)
	{
		return Lhs->OrderIndependentCompareEqual(*Rhs);
	}
	uint32 SetStoreKeyFuncs::GetKeyHash(const TContext* Key)
	{
		return HashProperties(*Key);
	}

	//
	// PropertySetStore
	//
	PropertySetStore::PropertySetStore()
	{
	}

	PropertySetStore::~PropertySetStore()
	{
	}

	PropertySet* PropertySetStore::Get(const TContext& Context)
	{
		FScopeLock Lock(&Mutex);
		const FSetElementId SetStorageIndex = PropertySets.FindId(&Context);
		if (SetStorageIndex.IsValidId())
		{
			return PropertySets[SetStorageIndex];
		}
		else
		{
			PropertySet* NewSet = new PropertySet(Context);
			PropertySets.Add(NewSet);
			return NewSet;
		}
	}
} // namespace MicromegasTracing
