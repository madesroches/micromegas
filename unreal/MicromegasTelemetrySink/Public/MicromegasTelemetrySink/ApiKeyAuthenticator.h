#pragma once
//
// MicromegasTelemetrySink/ApiKeyAuthenticator.h
//
// Generic API key authenticator for telemetry.
// Accepts API key via constructor, sends as Bearer token.
//

#include "Containers/UnrealString.h"
#include "MicromegasTelemetrySink/TelemetryAuthenticator.h"

/**
 * Simple API key authenticator.
 * API key is provided at construction time.
 * Always ready, signs requests with Bearer token.
 */
class MICROMEGASTELEMETRYSINK_API FApiKeyAuthenticator : public ITelemetryAuthenticator
{
public:
	explicit FApiKeyAuthenticator(const FString& InApiKey);
	virtual ~FApiKeyAuthenticator();

	// ITelemetryAuthenticator
	virtual void Init(const MicromegasTracing::EventSinkPtr& InSink) override;
	virtual bool IsReady() override;
	virtual bool Sign(IHttpRequest& Request) override;

private:
	FString ApiKey;
};
