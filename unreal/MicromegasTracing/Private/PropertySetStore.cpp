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
			uint32 Hash = 0;
			for (const TPair<FName, FName>& prop : Context)
			{
				Hash = HashCombine(GetTypeHash(prop.Key), Hash);
				Hash = HashCombine(GetTypeHash(prop.Value), Hash);
			}
			return Hash;
		}
	} // namespace

	//
	// SetStoreKeyFuncs
	//
	const TContext* SetStoreKeyFuncs::GetSetKey(PropertySet* Properties)
	{
		return &Properties->GetContext();
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
			// leaked by design, be careful of the cardinality
			PropertySet* NewSet = new PropertySet(Context);
			PropertySets.Add(NewSet);
			return NewSet;
		}
	}
} // namespace MicromegasTracing
