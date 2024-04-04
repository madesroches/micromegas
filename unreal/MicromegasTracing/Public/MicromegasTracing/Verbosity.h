#pragma once
//
//  MicromegasTracing/Verbosity.h
//
namespace MicromegasTracing
{
	namespace Verbosity
	{
		enum Type : uint8
		{
			Min = 1, // low frequency
			Med = 2, // frame frequency
			Max = 3, // many instances per frame
		};
	} // namespace Verbosity
} // namespace MicromegasTracing
