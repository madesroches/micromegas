#pragma once
//
//  LgnTelemetrySink/ThreadDependencies.h
//
#include "LgnTracing/ThreadMetadata.h"

typedef LgnTracing::HeterogeneousQueue<
    LgnTracing::StaticStringDependency,
    LgnTracing::SpanMetadataDependency > ThreadDependenciesQueue;

struct ExtractThreadDependencies
{
    TSet<const void*> Ids;
    ThreadDependenciesQueue Dependencies;

    ExtractThreadDependencies()
        : Dependencies( 1024*1024 )
    {
    }

    void operator()( const LgnTracing::StaticStringRef& str )
    {
        bool alreadyInSet = false;
        Ids.Add( reinterpret_cast<void*>(str.GetID()), &alreadyInSet );
        if ( !alreadyInSet )
        {
            Dependencies.Push( LgnTracing::StaticStringDependency( str ) );
        }
    }

    void operator()( const LgnTracing::SpanMetadata* desc )
    {
        bool alreadyInSet = false;
        Ids.Add( desc, &alreadyInSet );
        if ( !alreadyInSet )
        {
            (*this)( LgnTracing::StaticStringRef( desc->Name ) );
            (*this)( LgnTracing::StaticStringRef( desc->Target ) );
            (*this)( LgnTracing::StaticStringRef( desc->File ) );
            Dependencies.Push( LgnTracing::SpanMetadataDependency( desc ) );
        }
    }

    void operator()( const LgnTracing::BeginThreadSpanEvent& event )
    {
        (*this)( event.Desc );
    }

    void operator()( const LgnTracing::EndThreadSpanEvent& event )
    {
        (*this)( event.Desc );
    }
    
    ExtractThreadDependencies(const ExtractThreadDependencies&) = delete;
    ExtractThreadDependencies& operator=( const ExtractThreadDependencies&) = delete;
};
