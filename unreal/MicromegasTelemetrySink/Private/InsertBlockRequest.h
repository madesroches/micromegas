#pragma once
//
//  MicromegasTelemetrySink/InsertBlockRequest.h
//
#include "Dom/JsonObject.h"
#include "Dom/JsonValue.h"
#include "FormatTime.h"
#include "LogDependencies.h"
#include "MetricDependencies.h"
#include "MicromegasTelemetrySink/Log.h"
#include "MicromegasTracing/LogBlock.h"
#include "MicromegasTracing/MetricBlock.h"
#include "Serialization/JsonSerializer.h"
#include "Serialization/JsonWriter.h"
#include "ThreadDependencies.h"

TArray<uint8> CompressBuffer(const void* src, size_t size);

TUniquePtr<ExtractLogDependencies>	  ExtractBlockDependencies(const MicromegasTracing::LogBlock& block);
TUniquePtr<ExtractMetricDependencies> ExtractBlockDependencies(const MicromegasTracing::MetricBlock& block);
TUniquePtr<ExtractThreadDependencies> ExtractBlockDependencies(const MicromegasTracing::ThreadBlock& block);

template <typename BlockT>
inline TArray<uint8> FormatBlockRequest(const BlockT& block)
{
	MICROMEGAS_SPAN_SCOPE(TEXT("MicromegasTelemetrySink"), TEXT("FormatBlockRequest"));
	using namespace MicromegasTracing;
	auto& queue = block.GetEvents();

	auto depExtrator = ExtractBlockDependencies(block);

	FString					blockId = FGuid::NewGuid().ToString(EGuidFormats::DigitsWithHyphens);
	TSharedRef<FJsonObject> blockInfo = MakeShareable(new FJsonObject);
	blockInfo->SetStringField(TEXT("block_id"), *blockId);
	blockInfo->SetStringField(TEXT("stream_id"), block.GetStreamId().c_str());
	blockInfo->SetStringField(TEXT("begin_time"), FormatTimeIso8601(block.GetBeginTime()).c_str());
	blockInfo->SetStringField(TEXT("begin_ticks"), std::to_string(block.GetBeginTime().Timestamp).c_str());
	blockInfo->SetStringField(TEXT("end_time"), FormatTimeIso8601(block.GetEndTime()).c_str());
	blockInfo->SetStringField(TEXT("end_ticks"), std::to_string(block.GetEndTime().Timestamp).c_str());
	blockInfo->SetStringField(TEXT("nb_objects"), std::to_string(block.GetEvents().GetNbEvents()).c_str());

	FString					  jsonText;
	TSharedRef<TJsonWriter<>> jsonWriter = TJsonWriterFactory<>::Create(&jsonText);
	if (!FJsonSerializer::Serialize(blockInfo, jsonWriter))
	{
		UE_LOG(LogMicromegasTelemetrySink, Error, TEXT("Error formatting block info as json"));
		return TArray<uint8>();
	}
	jsonWriter->Close();

	std::vector<uint8> buffer;
	DynamicString	   blockInfoDynStr(*jsonText);
	buffer.reserve(Serializer<DynamicString>::GetSize(blockInfoDynStr));
	Serializer<DynamicString>::Write(blockInfoDynStr, buffer);

	TArray<uint8> compressedDep = CompressBuffer(
		depExtrator->Dependencies.GetPtr(),
		depExtrator->Dependencies.GetSizeBytes());
	details::WritePOD(static_cast<uint32>(compressedDep.Num()), buffer);
	if (compressedDep.Num() > 0)
	{
		buffer.insert(buffer.end(), compressedDep.GetData(), compressedDep.GetData() + compressedDep.Num());
	}

	TArray<uint8> compressedObj = CompressBuffer(queue.GetPtr(), queue.GetSizeBytes());
	details::WritePOD(static_cast<uint32>(compressedObj.Num()), buffer);
	if (compressedObj.Num() > 0)
	{
		buffer.insert(buffer.end(), compressedObj.GetData(), compressedObj.GetData() + compressedObj.Num());
	}
	return TArray<uint8>(&buffer[0], buffer.size());
}
