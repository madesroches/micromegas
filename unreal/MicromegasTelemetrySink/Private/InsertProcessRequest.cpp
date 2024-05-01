#pragma once
//
//  MicromegasTelemetrySink/InsertProcessRequest.cpp
//
#include "InsertProcessRequest.h"
#include "CborUtils.h"
#include "FormatTime.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/ProcessInfo.h"

TArray<uint8> FormatInsertProcessRequest(const MicromegasTracing::ProcessInfo& processInfo)
{
	std::vector<uint8> buffer;
	jsoncons::cbor::cbor_bytes_encoder encoder(buffer);
	{
		encoder.begin_object();
		encoder.key("process_id");
		encode_utf8_string(encoder, *processInfo.ProcessId);
		encoder.key("parent_process_id");
		encode_utf8_string(encoder, *processInfo.ParentProcessId);
		encoder.key("exe");
		encode_utf8_string(encoder, *processInfo.Exe);
		encoder.key("username");
		encode_utf8_string(encoder, *processInfo.Username);
		encoder.key("realname");
		encode_utf8_string(encoder, *processInfo.Username);
		encoder.key("computer");
		encode_utf8_string(encoder, *processInfo.Computer);
		encoder.key("distro");
		encode_utf8_string(encoder, *processInfo.Distro);
		encoder.key("cpu_brand");
		encode_utf8_string(encoder, *processInfo.CpuBrand);
		encoder.key("tsc_frequency");
		encoder.int64_value(processInfo.TscFrequency);
		encoder.key("start_time");
		encode_utf8_string(encoder, *FormatTimeIso8601(processInfo.StartTime));
		encoder.key("start_ticks");
		encoder.int64_value(processInfo.StartTime.Timestamp);
		encoder.key("properties");
		encoder.begin_object();
		for (const auto& kv : processInfo.Properties)
		{
			FTCHARToUTF8 UTF8Key(*kv.Key);
			using string_view_type = jsoncons::cbor::cbor_bytes_encoder::string_view_type;
			encoder.key(string_view_type(UTF8Key.Get(), UTF8Key.Length()));
			encode_utf8_string(encoder, *kv.Value);
		}
		encoder.end_object();
		
		encoder.end_object();
	}
	encoder.flush();
	return TArray<uint8>(&buffer[0], buffer.size());
}
