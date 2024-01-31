#pragma once
//
//  LgnTelemetrySink/MetricDependencies.h
//
#include "LgnTracing/MetricEvents.h"
typedef LgnTracing::HeterogeneousQueue<
     LgnTracing::StaticStringDependency,
     LgnTracing::MetricMetadataDependency> MetricDependenciesQueue;

struct ExtractMetricDependencies
{
    TSet<const void*> Ids;
    MetricDependenciesQueue Dependencies;

    ExtractMetricDependencies()
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

    void operator()( const LgnTracing::MetricMetadata* metricDesc )
    {
        bool alreadyInSet = false;
        Ids.Add( metricDesc, &alreadyInSet );
        if ( !alreadyInSet )
        {
            (*this)( LgnTracing::StaticStringRef( metricDesc->Name ) );
            (*this)( LgnTracing::StaticStringRef( metricDesc->Unit ) );
            (*this)( LgnTracing::StaticStringRef( metricDesc->Target ) );
            (*this)( LgnTracing::StaticStringRef( metricDesc->File ) );
            Dependencies.Push( LgnTracing::MetricMetadataDependency( metricDesc ) );
        }
    }

    void operator()( const LgnTracing::IntegerMetricEvent& event )
    {
        (*this)( event.Desc );
    }

    void operator()( const LgnTracing::FloatMetricEvent& event )
    {
        (*this)( event.Desc );
    }
    
    ExtractMetricDependencies(const ExtractMetricDependencies&) = delete;
    ExtractMetricDependencies& operator=( const ExtractMetricDependencies&) = delete;
};
