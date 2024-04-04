#pragma once
//
//  MicromegasTelemetrySink/ThreadDependencies.h
//
#include "MicromegasTracing/ThreadMetadata.h"

typedef MicromegasTracing::HeterogeneousQueue<
    MicromegasTracing::StaticStringDependency,
    MicromegasTracing::SpanMetadataDependency > ThreadDependenciesQueue;

struct ExtractThreadDependencies
{
    TSet<const void*> Ids;
    ThreadDependenciesQueue Dependencies;

    ExtractThreadDependencies()
        : Dependencies( 1024*1024 )
    {
    }

    void operator()( const MicromegasTracing::StaticStringRef& str )
    {
        bool alreadyInSet = false;
        Ids.Add( reinterpret_cast<void*>(str.GetID()), &alreadyInSet );
        if ( !alreadyInSet )
        {
            Dependencies.Push( MicromegasTracing::StaticStringDependency( str ) );
        }
    }

    void operator()( const MicromegasTracing::SpanMetadata* desc )
    {
        bool alreadyInSet = false;
        Ids.Add( desc, &alreadyInSet );
        if ( !alreadyInSet )
        {
            (*this)( MicromegasTracing::StaticStringRef( desc->Name ) );
            (*this)( MicromegasTracing::StaticStringRef( desc->Target ) );
            (*this)( MicromegasTracing::StaticStringRef( desc->File ) );
            Dependencies.Push( MicromegasTracing::SpanMetadataDependency( desc ) );
        }
    }

    void operator()( const MicromegasTracing::BeginThreadSpanEvent& event )
    {
        (*this)( event.Desc );
    }

    void operator()( const MicromegasTracing::EndThreadSpanEvent& event )
    {
        (*this)( event.Desc );
    }
    
    ExtractThreadDependencies(const ExtractThreadDependencies&) = delete;
    ExtractThreadDependencies& operator=( const ExtractThreadDependencies&) = delete;
};
