//
//  MicromegasTracing/DefaultContext.cpp
//
#include "MicromegasTracing/DefaultContext.h"
#include "MicromegasTracing/Dispatch.h"
#include "MicromegasTracing/PropertySetStore.h"
#include "Misc/ScopeLock.h"

namespace MicromegasTracing
{
	DefaultContext::DefaultContext(PropertySetStore* InStore)
		: Store(InStore)
	{
		check(Store);
		UpdatePropertySet();
	}

	DefaultContext::~DefaultContext() {}

	void DefaultContext::Set(FName Key, FName Value)
	{
		FScopeLock Lock(&Mutex);
		FName* StoredValue = Context.Find(Key);
		if (StoredValue != nullptr)
		{
			if (*StoredValue == Value)
			{
				return;
			}
			else
			{
				*StoredValue = Value;
			}
		}
		else
		{
			Context.Add(Key, Value);
		}

		UpdatePropertySet();
	}

	void DefaultContext::Unset(FName Key)
	{
		FScopeLock Lock(&Mutex);
		Context.Remove(Key);
		UpdatePropertySet();
	}

	void DefaultContext::Clear()
	{
		FScopeLock Lock(&Mutex);
		Context.Empty();
		UpdatePropertySet();
	}

	void DefaultContext::Copy(TMap<FName, FName>& Out)const
	{
		FScopeLock Lock(&Mutex);
		Out = Context;
	}

	void DefaultContext::UpdatePropertySet()
	{
		FPlatformMisc::MemoryBarrier(); // make sure property set is flushed before it's available to other threads
		CurrentPropertySet = Store->Get(Context);
	}

} // namespace MicromegasTracing
