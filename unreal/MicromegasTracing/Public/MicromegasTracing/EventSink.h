#pragma once
//
//  MicromegasTracing/EventSink.h
//
#include "HAL/Platform.h"
#include "MicromegasTracing/Fwd.h"

namespace MicromegasTracing
{
    class CORE_API EventSink
    {
    public:
        virtual ~EventSink() = 0;
        virtual void OnStartup( const ProcessInfoPtr& processInfo ) = 0;
        virtual void OnShutdown() = 0;

        virtual void OnInitLogStream( const LogStreamPtr& stream ) = 0;
        virtual void OnInitMetricStream( const MetricStreamPtr& stream ) = 0;
        virtual void OnInitThreadStream( ThreadStream* stream ) = 0;

        virtual void OnProcessLogBlock( const LogBlockPtr& block ) = 0;
        virtual void OnProcessMetricBlock( const MetricsBlockPtr& block ) = 0;
        virtual void OnProcessThreadBlock( const ThreadBlockPtr& block ) = 0;

        virtual bool IsBusy() = 0;
    };
} // namespace
