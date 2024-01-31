#pragma once
//
//  LgnTelemetrySink/LogDependencies.h
//
#include "LgnTracing/HeterogeneousQueue.h"
#include "LgnTracing/strings.h"
#include "LgnTracing/LogEvents.h"
#include "LgnTracing/StaticStringDependency.h"

typedef LgnTracing::HeterogeneousQueue<LgnTracing::StaticStringDependency,LgnTracing::LogMetadataDependency> LogDependenciesQueue;

struct ExtractLogDependencies
{
    TSet<const void*> Ids;
    LogDependenciesQueue Dependencies;

    ExtractLogDependencies()
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

    void operator()( const LgnTracing::LogMetadata* logDesc )
    {
        bool alreadyInSet = false;
        Ids.Add( logDesc, &alreadyInSet );
        if ( !alreadyInSet )
        {
            (*this)( LgnTracing::StaticStringRef( logDesc->Target ) );
            (*this)( LgnTracing::StaticStringRef( logDesc->Msg ) );
            (*this)( LgnTracing::StaticStringRef( logDesc->File ) );
            Dependencies.Push( LgnTracing::LogMetadataDependency( logDesc ) );
        }
    }
    
    void operator()( const LgnTracing::LogStringInteropEvent& evt )
    {
        (*this)( evt.Target );
    }

    void operator()( const LgnTracing::LogStaticStrEvent& evt )
    {
        (*this)( evt.Desc );
    }

    ExtractLogDependencies(const ExtractLogDependencies&) = delete;
    ExtractLogDependencies& operator=( const ExtractLogDependencies&) = delete;
};
