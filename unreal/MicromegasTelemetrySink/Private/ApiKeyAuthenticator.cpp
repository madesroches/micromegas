//
// MicromegasTelemetrySink/ApiKeyAuthenticator.cpp
//

#include "MicromegasTelemetrySink/ApiKeyAuthenticator.h"
#include "MicromegasTelemetrySink/Log.h"
#include "Interfaces/IHttpRequest.h"

FApiKeyAuthenticator::FApiKeyAuthenticator(const FString& InApiKey)
	: ApiKey(InApiKey)
{
	if (ApiKey.IsEmpty())
	{
		UE_LOG(LogMicromegasTelemetrySink, Warning, TEXT("API key is empty"));
	}
	else
	{
		UE_LOG(LogMicromegasTelemetrySink, Log, TEXT("Using API key authentication"));
	}
}

FApiKeyAuthenticator::~FApiKeyAuthenticator() = default;

void FApiKeyAuthenticator::Init(const MicromegasTracing::EventSinkPtr& InSink)
{
	// No sink dependency needed for API key auth
}

bool FApiKeyAuthenticator::IsReady()
{
	return !ApiKey.IsEmpty();
}

bool FApiKeyAuthenticator::Sign(IHttpRequest& Request)
{
	if (ApiKey.IsEmpty())
	{
		return false;
	}
	Request.SetHeader(TEXT("Authorization"), FString::Printf(TEXT("Bearer %s"), *ApiKey));
	return true;
}
