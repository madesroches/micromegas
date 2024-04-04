#pragma once
//
//  MicromegasTelemetrySink/InsertProcessRequest.cpp
//
#include "InsertProcessRequest.h"
#include "MicromegasTelemetrySink/Log.h"
#include "Dom/JsonValue.h"
#include "Dom/JsonObject.h"
#include "Serialization/JsonWriter.h"
#include "Serialization/JsonSerializer.h"
#include "MicromegasTracing/ProcessInfo.h"
#include "FormatTime.h"

TSharedPtr<FJsonObject> FormatProcessInfo(const MicromegasTracing::ProcessInfo& processInfo)
{
	using namespace MicromegasTracing;
	TSharedPtr<FJsonObject> obj = MakeShareable(new FJsonObject);
	obj->SetStringField(TEXT("process_id"), processInfo.ProcessId.c_str());
	obj->SetStringField(TEXT("parent_process_id"), processInfo.ParentProcessId.c_str());
	obj->SetStringField(TEXT("exe"), processInfo.Exe.c_str());
	obj->SetStringField(TEXT("username"), processInfo.Username.c_str());
	obj->SetStringField(TEXT("realname"), processInfo.Username.c_str());
	obj->SetStringField(TEXT("computer"), processInfo.Computer.c_str());
	obj->SetStringField(TEXT("distro"), processInfo.Distro.c_str());
	obj->SetStringField(TEXT("cpu_brand"), processInfo.CpuBrand.c_str());
	obj->SetStringField(TEXT("tsc_frequency"), std::to_string(processInfo.TscFrequency).c_str());
	obj->SetStringField(TEXT("start_time"), FormatTimeIso8601(processInfo.StartTime).c_str());
	obj->SetStringField(TEXT("start_ticks"), std::to_string(processInfo.StartTime.Timestamp).c_str());
	return obj;
}

FString FormatInsertProcessRequest(const MicromegasTracing::ProcessInfo& processInfo)
{
	FString					  jsonText;
	TSharedRef<TJsonWriter<>> jsonWriter = TJsonWriterFactory<>::Create(&jsonText);
	TSharedPtr<FJsonObject>	  obj = FormatProcessInfo(processInfo);
	if (!FJsonSerializer::Serialize(obj.ToSharedRef(), jsonWriter))
	{
		UE_LOG(LogMicromegasTelemetrySink, Error, TEXT("Error formatting processInfo as json"));
		return TEXT("");
	}
	jsonWriter->Close();
	return jsonText;
}
