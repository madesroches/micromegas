#pragma once
//
//  MicromegasTracing/DualTime.h
//
#include <string>
#include <chrono>
#include "HAL/PlatformTime.h"

namespace MicromegasTracing
{
    struct DualTime
    {
        uint64 Timestamp;

        typedef std::chrono::time_point<std::chrono::system_clock> SystemTimeT;
        SystemTimeT SystemTime;

        DualTime()
            : Timestamp( 0 )
        {
        }

        DualTime( uint64 timestamp, const SystemTimeT& systemTime)
            : Timestamp( timestamp )
            , SystemTime( systemTime )
            
        {
        }

        static DualTime Now()
        {
            return DualTime( FPlatformTime::Cycles64(), std::chrono::system_clock::now() );
        }
    };
}
