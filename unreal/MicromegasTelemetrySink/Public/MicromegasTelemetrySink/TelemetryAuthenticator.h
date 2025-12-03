#pragma once
//
// MicromegasTelemetrySink/TelemetryAuthenticator.h
//
#include "MicromegasTracing/Fwd.h"
#include "Templates/SharedPointerFwd.h"

class IHttpRequest;

class MICROMEGASTELEMETRYSINK_API ITelemetryAuthenticator
{
public:
	virtual ~ITelemetryAuthenticator() = 0;
	virtual void Init(const MicromegasTracing::EventSinkPtr& InSink) = 0;
	virtual void Shutdown();
	virtual bool IsReady() = 0;
	virtual bool Sign(IHttpRequest& Request) = 0;
};

typedef TSharedPtr<ITelemetryAuthenticator, ESPMode::ThreadSafe> SharedTelemetryAuthenticator;
