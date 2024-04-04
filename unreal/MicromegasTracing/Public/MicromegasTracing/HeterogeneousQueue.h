#pragma once
//
//  MicromegasTracing/HeterogeneousQueue.h
//
#include <vector>
#include <cassert>
#include "MicromegasTracing/Macros.h"

namespace MicromegasTracing
{
    namespace details
    {
        template< typename TO_FIND, typename... TS >
        struct IndexOfType;
    
        template< typename TO_FIND, typename HEAD, typename... REST >
        struct IndexOfType< TO_FIND, HEAD, REST... >
        {
            static const uint8 value = 1 + IndexOfType<TO_FIND, REST...>::value;
        };

        template< typename TO_FIND, typename... REST >
        struct IndexOfType< TO_FIND, TO_FIND, REST... >
        {
            static const uint8 value = 0;
        };

        template< typename T >
        const T& ReadPOD( const std::vector<uint8>& buffer, size_t& cursor )
        {
            size_t indexToRead = cursor;
            cursor += sizeof(T);
            return *reinterpret_cast<const T*>(&buffer[0] + indexToRead);
        }

        template< typename T >
        void WritePOD( const T& value, std::vector<uint8>& buffer )
        {
            const uint8* beginBytes = reinterpret_cast<const uint8*>( &value );
            buffer.insert(buffer.end(), beginBytes, beginBytes + sizeof(T));
        }
    }

    template< typename T >
    struct Serializer
    {
        static bool IsSizeStatic()
        {
            return true;
        }

        static uint32 GetSize( const T& value )
        {
            return sizeof(T);
        }
    
        static void Write( const T& value, std::vector<uint8>& buffer )
        {
            details::WritePOD( value, buffer );
        }

        template< typename Callback >
        static void Read( Callback& callback, const std::vector<uint8>& buffer, size_t& cursor )
        {
            callback( *reinterpret_cast<const T*>( &buffer[0] + cursor ) );
            cursor += sizeof(T);
        }
    };

    template< typename... TS >
    class HeterogeneousQueue
    {
    public:
        explicit HeterogeneousQueue( size_t bufferSize )
            :NbEvents(0)
        {
            Buffer.reserve( bufferSize );
        }

        size_t GetSizeBytes()const
        {
            return Buffer.size();
        }

        template< typename T >
        void Push( const T& event )
        {
            ++NbEvents;
            uint8 typeIndex = details::IndexOfType<T, TS...>::value;
            Buffer.push_back(typeIndex);
            if ( !Serializer<T>::IsSizeStatic() )
            {
                details::WritePOD(Serializer<T>::GetSize(event), Buffer);
            }
            Serializer<T>::Write(event, Buffer);
        }

        template< typename Visitor >
        void ForEach( Visitor& v )const
        {
            MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTracing"), TEXT("HeterogeneousQueue::ForEach"));
            size_t cursor = 0;
            while ( cursor < GetSizeBytes() )
            {
                uint8 typeIndex = details::ReadPOD<uint8>(Buffer, cursor);
                VisitValue<Visitor, TS...>(typeIndex, v, cursor);
            }
        }

        size_t GetNbEvents()const
        {
            return NbEvents;
        }

        const uint8* GetPtr()const
        {
            return &Buffer[0];
        }
    
    private:

        template< typename Visitor, typename HEAD, typename... REST >
        void VisitValue( uint8 typeIndex, Visitor& v, size_t& cursor )const
        {
            if ( 0 == typeIndex )
            {
                uint32 valueSize = 0;
                if ( !Serializer<HEAD>::IsSizeStatic() )
                {
                    valueSize = details::ReadPOD<uint32>(Buffer, cursor);
                }
                Serializer<HEAD>::Read(v, Buffer, cursor);
            }
            else
            {
                VisitValue<Visitor, REST...>(typeIndex-1, v, cursor);
            }
        }

        template< typename Visitor >
        void VisitValue( uint8 typeIndex, Visitor& v, size_t& cursor )const
        {
            assert(!"type not found");
        }
    
        std::vector<uint8> Buffer;
        size_t NbEvents;
    };

} // namespace
