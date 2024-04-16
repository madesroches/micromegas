#pragma once
//
// MicromegasTelemetrySink/TelemetryAuthenticator.h
//

#include "Templates/SharedPointerFwd.h"

class IHttpRequest;

class MICROMEGASTELEMETRYSINK_API ITelemetryAuthenticator
{
public:
	virtual ~ITelemetryAuthenticator() = 0;
	virtual bool IsReady() = 0;
	virtual bool Sign(IHttpRequest& request) = 0;
};

typedef TSharedRef<ITelemetryAuthenticator, ESPMode::ThreadSafe> SharedTelemetryAuthenticator;
