#pragma once
//
//  MicromegasTracing/ImageMetadata.h
//
#include "MicromegasTracing/ImageEvents.h"
#include "MicromegasTracing/QueueMetadata.h"

namespace MicromegasTracing
{
	template <>
	struct GetEventMetadata<ImageEvent>
	{
		UserDefinedType operator()()
		{
			return UserDefinedType(TEXT("ImageEvent"), 0, false, {});
		}
	};
} // namespace MicromegasTracing
