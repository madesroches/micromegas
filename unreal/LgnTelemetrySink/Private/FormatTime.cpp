//
//  FormatTime.cpp
//
#include "FormatTime.h"
#include "LgnTracing/DualTime.h"
#include <sstream>
#include <iomanip>

std::string FormatTimeIso8601( const LgnTracing::DualTime& dualTime )
{
    std::time_t time = std::chrono::system_clock::to_time_t(dualTime.SystemTime);
    tm utcTime;
    gmtime_s(&utcTime, &time);
    std::ostringstream str;
    str << std::put_time(&utcTime, "%FT%TZ");
    return str.str();
}
