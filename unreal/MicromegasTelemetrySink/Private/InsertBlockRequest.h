#pragma once
//
//  MicromegasTelemetrySink/InsertBlockRequest.h
//
#include "CborUtils.h"
#include "FormatTime.h"
#include "LogDependencies.h"
#include "MetricDependencies.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/MetricBlock.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "Misc/Guid.h"
#include "ThreadDependencies.h"

std::vector<uint8> CompressBuffer(const void* src, size_t size);

TUniquePtr<ExtractLogDependencies> ExtractBlockDependencies(const MicromegasTracing::LogBlock& block);
TUniquePtr<ExtractMetricDependencies> ExtractBlockDependencies(const MicromegasTracing::MetricBlock& block);
TUniquePtr<ExtractThreadDependencies> ExtractBlockDependencies(const MicromegasTracing::ThreadBlock& block);

template <typename BlockT>
inline TArray<uint8> FormatBlockRequest(const MicromegasTracing::ProcessInfo& processInfo, const BlockT& block)
{
	MICROMEGAS_SPAN_FUNCTION("MicromegasTelemetrySink");
	using namespace MicromegasTracing;
	auto& queue = block.GetEvents();

	auto depExtrator = ExtractBlockDependencies(block);

	FString blockId = FGuid::NewGuid().ToString(EGuidFormats::DigitsWithHyphens);
	MICROMEGAS_LOG("LogMicromegasTelemetrySink", MicromegasTracing::LogLevel::Debug, FString::Printf(TEXT("Sending block %s"), *blockId));

	std::vector<uint8> compressedDep = CompressBuffer(
		depExtrator->Dependencies.GetPtr(),
		depExtrator->Dependencies.GetSizeBytes());
	std::vector<uint8> compressedObj = CompressBuffer(queue.GetPtr(), queue.GetSizeBytes());

	std::vector<uint8> buffer;
	jsoncons::cbor::cbor_bytes_encoder encoder(buffer);
	{
		encoder.begin_object();
		encoder.key("block_id");
		encode_utf8_string(encoder, *blockId);
		encoder.key("stream_id");
		encode_utf8_string(encoder, *block.GetStreamId());
		encoder.key("process_id");
		encode_utf8_string(encoder, *processInfo.ProcessId);
		encoder.key("begin_time");
		encode_utf8_string(encoder, *FormatTimeIso8601(block.GetBeginTime()));
		encoder.key("begin_ticks");
		encoder.int64_value(block.GetBeginTime().Timestamp - processInfo.StartTime.Timestamp);
		encoder.key("end_time");
		encode_utf8_string(encoder, *FormatTimeIso8601(block.GetEndTime()));
		encoder.key("end_ticks");
		encoder.int64_value(block.GetEndTime().Timestamp - processInfo.StartTime.Timestamp);
		encoder.key("payload");
		{
			encoder.begin_object();
			encoder.key("dependencies");
			encoder.byte_string_value(compressedDep);
			encoder.key("objects");
			encoder.byte_string_value(compressedObj);
			encoder.end_object();
		}
		encoder.key("nb_objects");
		encoder.int64_value(block.GetEvents().GetNbEvents());
		encoder.key("object_offset");
		encoder.int64_value(block.GetOffset());
		encoder.end_object();
	}
	encoder.flush();

	return TArray<uint8>(&buffer[0], buffer.size());
}
