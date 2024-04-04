#pragma once
//
//  MicromegasTracing/StringsMetadata.h
//
#include "MicromegasTracing/strings.h"

namespace MicromegasTracing
{
    template< typename T >
    struct GetEventMetadata;
    
    template<>
    struct GetEventMetadata< StaticStringRef >
    {
        UserDefinedType operator()()
        {
            return UserDefinedType(
                TEXT("StaticStringRef"),
                sizeof(StaticStringRef),
                true, // object is a reference
                {
                    MAKE_UDT_MEMBER_METADATA(StaticStringRef, "id", Ptr, uint64, true),
                } );
        }
    };
}

