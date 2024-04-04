//
//  MicromegasTelemetrySink/InsertBlockRequest.cpp
//
#include "InsertBlockRequest.h"
#include "MicromegasLz4/lz4frame.h"


TArray<uint8> CompressBuffer( const void* src, size_t size )
{
    MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("CompressBuffer"));
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


TUniquePtr<ExtractLogDependencies> ExtractBlockDependencies( const MicromegasTracing::LogBlock& block )
{
    MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractLogDependencies> extractDependencies( new ExtractLogDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}

TUniquePtr<ExtractMetricDependencies> ExtractBlockDependencies( const MicromegasTracing::MetricBlock& block )
{
    MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractMetricDependencies> extractDependencies( new ExtractMetricDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}

TUniquePtr<ExtractThreadDependencies> ExtractBlockDependencies( const MicromegasTracing::ThreadBlock& block )
{
    MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("ExtractBlockDependencies"));
    TUniquePtr<ExtractThreadDependencies> extractDependencies( new ExtractThreadDependencies() );
    block.GetEvents().ForEach( *extractDependencies );
    return extractDependencies;
}
