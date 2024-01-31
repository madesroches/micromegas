#pragma once
//
//  LgnTelemetrySink/InsertProcessRequest.h
//

namespace LgnTracing
{
    struct ProcessInfo;
}

FString FormatInsertProcessRequest(const LgnTracing::ProcessInfo& processInfo);
