#pragma once
//
//  LgnTracing/MetricEvents.h
//
#include "LgnTracing/Verbosity.h"

namespace LgnTracing
{
    struct MetricMetadata
    {
        Verbosity::Type Lod;
        const TCHAR* Name;
        const TCHAR* Unit;
        const TCHAR* Target;
        const TCHAR* File;
        uint32 Line;

        MetricMetadata( Verbosity::Type lod,
                        const TCHAR* name,
                        const TCHAR* unit,
                        const TCHAR* target,
                        const TCHAR* file,
                        uint32 line )
            : Lod( lod )
            , Name( name )
            , Unit( unit )
            , Target( target )
            , File( file )
            , Line( line )
        {
        }
    };

    struct IntegerMetricEvent
    {
        const MetricMetadata* Desc;
        uint64 Value;
        uint64 Timestamp;

        IntegerMetricEvent( const MetricMetadata* desc, uint64 value, uint64 timestamp )
            : Desc( desc )
            , Value( value )
            , Timestamp( timestamp )
        {
        }
    };

    struct FloatMetricEvent
    {
        const MetricMetadata* Desc;
        double Value;
        uint64 Timestamp;

        FloatMetricEvent( const MetricMetadata* desc, double value, uint64 timestamp )
            : Desc( desc )
            , Value( value )
            , Timestamp( timestamp )
        {
        }
    };

    struct MetricMetadataDependency
    {
        uint64 Id;
        Verbosity::Type Lod;
        const TCHAR* Name;
        const TCHAR* Unit;
        const TCHAR* Target;
        const TCHAR* File;
        uint32 Line;

        explicit MetricMetadataDependency( const MetricMetadata* mm )
            : Id( reinterpret_cast<uint64>( mm ) )
            , Lod( mm->Lod )
            , Name( mm->Name )
            , Unit( mm->Unit )
            , Target( mm->Target )
            , File( mm->File )
            , Line( mm->Line )
        {
        }
    };
}

