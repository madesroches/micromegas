#pragma once
//
//  MicromegasTelemetrySink/InsertProcessRequest.cpp
//
#include "CborUtils.h"
#include "Dom/JsonObject.h"
#include "Dom/JsonValue.h"
#include "FormatTime.h"
#include "InsertProcessRequest.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "Serialization/JsonSerializer.h"
#include "Serialization/JsonWriter.h"

TArray<uint8> FormatInsertProcessRequest(const MicromegasTracing::ProcessInfo& processInfo)
{
	std::vector<uint8> buffer;
	jsoncons::cbor::cbor_bytes_encoder encoder(buffer);
	{
		encoder.begin_object();
		encoder.key("process_id");
		encode_utf8_string(encoder, processInfo.ProcessId.c_str());
		encoder.key("parent_process_id");
		encode_utf8_string(encoder, processInfo.ParentProcessId.c_str());
		encoder.key("exe");
		encode_utf8_string(encoder, processInfo.Exe.c_str());
		encoder.key("username");
		encode_utf8_string(encoder, processInfo.Username.c_str());
		encoder.key("realname");
		encode_utf8_string(encoder, processInfo.Username.c_str());
		encoder.key("computer");
		encode_utf8_string(encoder, processInfo.Computer.c_str());
		encoder.key("distro");
		encode_utf8_string(encoder, processInfo.Distro.c_str());
		encoder.key("cpu_brand");
		encode_utf8_string(encoder, processInfo.CpuBrand.c_str());
		encoder.key("tsc_frequency");
		encoder.int64_value(processInfo.TscFrequency);
		encoder.key("start_time");
		encoder.string_value(FormatTimeIso8601(processInfo.StartTime));
		encoder.key("start_ticks");
		encoder.int64_value(processInfo.StartTime.Timestamp);
		encoder.end_object();
	}
	encoder.flush();
	return TArray<uint8>(&buffer[0], buffer.size());
}
