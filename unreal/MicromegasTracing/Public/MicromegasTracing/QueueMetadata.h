#pragma once
//
//  MicromegasTracing/QueueMetadata.h
//
#include "Containers/Array.h"
#include "MicromegasTracing/HeterogeneousQueue.h"

namespace MicromegasTracing
{
    struct UDTMember
    {
        const TCHAR* Name;
        const TCHAR* TypeName;
        uint32 Offset;
        uint32 Size;
        bool IsReference;

        UDTMember( const TCHAR* name,
                   const TCHAR* typeName,
                   uint32 offset,
                   uint32 size,
                   bool ref )
            : Name( name )
            , TypeName( typeName )
            , Offset( offset )
            , Size( size )
            , IsReference( ref )
        {
        }
    };

#define MAKE_UDT_MEMBER_METADATA(udt, reflectedName, memberName, memberType, isReference) UDTMember( TEXT(reflectedName), TEXT(#memberType), STRUCT_OFFSET(udt, memberName), sizeof(udt::memberName), isReference )

    struct UserDefinedType
    {
        const TCHAR* Name;
        uint32 Size;
        bool IsReference;
        TArray<UDTMember> Members;

        UserDefinedType( const TCHAR* name,
                         uint32 size,
                         bool isReference,
                         const TArray<UDTMember>& members)
            : Name( name )
            , Size( size )
            , IsReference( isReference )
            , Members( members )
        {
        }
    };

    template< typename T >
    struct GetEventMetadata;

    template< typename LAST >
    inline void AppendQueueMetadata( TArray< UserDefinedType >& array )
    {
        array.Push( GetEventMetadata<LAST>()() );
    }

    template< typename HEAD, typename NEXT, typename... REST >
    inline void AppendQueueMetadata( TArray< UserDefinedType >& array )
    {
        array.Push( GetEventMetadata<HEAD>()() );
        AppendQueueMetadata< NEXT, REST... >( array );
    }

    template< typename... TS >
    inline TArray< UserDefinedType > MakeTypeArrayMetadata( )
    {
        TArray< UserDefinedType > res;
        AppendQueueMetadata< TS... >( res );
        return res;
    }

    template< typename Q >
    struct MakeQueueMetadata;

    template< typename... TS >
    struct MakeQueueMetadata< HeterogeneousQueue<TS...> >
    {
        TArray< UserDefinedType > operator()()
        {
            TArray< UserDefinedType > res;
            AppendQueueMetadata< TS... >( res );
            return res;
        }
    };

} // namespace
