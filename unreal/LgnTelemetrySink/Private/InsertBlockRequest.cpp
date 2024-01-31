//
//  LgnTelemetrySink/InsertBlockRequest.cpp
//
#include "InsertBlockRequest.h"
#include "LgnLz4/lz4frame.h"


TArray<uint8> CompressBuffer( const void* src, size_t size )
{
    LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("CompressBuffer"));
    TArray<uint8> buffer;
    const int32 compressedBound = LZ4F_compressFrameBound(size, nullptr);
    buffer.AddUninitialized(compressedBound);
    uint32 compressedSize = LZ4F_compressFrame(
        buffer.GetData(),
        compressedBound,
        const_cast<void*>(src),
        size,
        nullptr);
    buffer.SetNum(compressedSize);
    return buffer;
}


TUniquePtr<ExtractLogDependencies> ExtractBlockDependencies( const LgnTracing::LogBlock& block )
{
    LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractLogDependencies> extractDependencies( new ExtractLogDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}

TUniquePtr<ExtractMetricDependencies> ExtractBlockDependencies( const LgnTracing::MetricBlock& block )
{
    LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractMetricDependencies> extractDependencies( new ExtractMetricDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}

TUniquePtr<ExtractThreadDependencies> ExtractBlockDependencies( const LgnTracing::ThreadBlock& block )
{
    LGN_SPAN_SCOPE(TEXT("LgnTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractThreadDependencies> extractDependencies( new ExtractThreadDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}
