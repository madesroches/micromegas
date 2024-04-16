#pragma once
//
//  MicromegasTelemetrySink/InsertStreamRequest.cpp
//
#include "InsertStreamRequest.h"
#include "MicromegasTelemetrySink/Log.h"
#include "LogDependencies.h"
#include <string>
#include "MicromegasTracing/QueueMetadata.h"
#include "MicromegasTracing/LogStream.h"
#include "MicromegasTracing/MetricMetadata.h"
#include "MicromegasTracing/StringsMetadata.h"
#include "MicromegasTracing/LogMetadata.h"
#include "MetricDependencies.h"
#include "ThreadDependencies.h"

void FormatContainerMetadata(jsoncons::cbor::cbor_bytes_encoder& encoder, const TArray<MicromegasTracing::UserDefinedType>& udts)
{
	encoder.begin_array();
	for (const MicromegasTracing::UserDefinedType& udt : udts)
	{
		encoder.begin_object();

		encoder.key("name");
		encode_utf8_string(encoder, udt.Name);

		encoder.key("size");
		encoder.uint64_value(udt.Size);

		encoder.key("is_reference");
		encoder.bool_value(udt.IsReference);

		encoder.key("members");
		encoder.begin_array();
		for (const MicromegasTracing::UDTMember& member : udt.Members)
		{
			encoder.begin_object();
			encoder.key("name");
			encode_utf8_string(encoder, member.Name);

			encoder.key("type_name");
			encode_utf8_string(encoder, member.TypeName);

			encoder.key("offset");
			encoder.uint64_value(member.Offset);

			encoder.key("size");
			encoder.uint64_value(member.Size);

			encoder.key("is_reference");
			encoder.bool_value(member.IsReference);

			encoder.end_object();
		}
		encoder.end_array();
		encoder.end_object();
	}
	encoder.end_array();
}

template <typename DepQueue, typename StreamT>
TArray<uint8> FormatInsertStreamRequest(const StreamT& stream)
{
	using namespace MicromegasTracing;

	std::vector<uint8> buffer;
	jsoncons::cbor::cbor_bytes_encoder encoder(buffer);
	{
		encoder.begin_object();
		encoder.key("stream_id");
		encode_utf8_string(encoder, *stream.GetStreamId());
		encoder.key("process_id");
		encode_utf8_string(encoder, *stream.GetProcessId());
		encoder.key("dependencies_metadata");
		FormatContainerMetadata(encoder, MakeQueueMetadata<DepQueue>()());
		encoder.key("objects_metadata");
		typedef typename StreamT::EventBlock EventBlock;
		typedef typename EventBlock::Queue EventQueue;
		FormatContainerMetadata(encoder, MakeQueueMetadata<EventQueue>()());

		encoder.key("tags");
		encoder.begin_array();
		for (const FString& tag : stream.GetTags())
		{
			encode_utf8_string(encoder, *tag);
		}
		encoder.end_array();

		encoder.key("properties");
		encoder.begin_object();
		for (const auto& kv : stream.GetProperties())
		{
			FTCHARToUTF8 UTF8Key(*kv.first);
			using string_view_type = jsoncons::cbor::cbor_bytes_encoder::string_view_type;
			encoder.key(string_view_type(UTF8Key.Get(), UTF8Key.Length()));
			encode_utf8_string(encoder, *kv.second);
		}
		encoder.end_object();

		encoder.end_object();
	}
	encoder.flush();

	return TArray<uint8>(&buffer[0], buffer.size());
}

TArray<uint8> FormatInsertLogStreamRequest(const MicromegasTracing::LogStream& stream)
{
	return FormatInsertStreamRequest<LogDependenciesQueue>(stream);
}

TArray<uint8> FormatInsertMetricStreamRequest(const MicromegasTracing::MetricStream& stream)
{
	return FormatInsertStreamRequest<MetricDependenciesQueue>(stream);
}

TArray<uint8> FormatInsertThreadStreamRequest(const MicromegasTracing::ThreadStream& stream)
{
	return FormatInsertStreamRequest<ThreadDependenciesQueue>(stream);
}
