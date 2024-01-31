#pragma once
//
//  LgnTelemetrySink/InsertStreamRequest.cpp
//
#include "InsertStreamRequest.h"
#include "LgnTelemetrySink/Log.h"
#include "LogDependencies.h"
#include "JsonUtils.h"
#include <string>
#include "LgnTracing/QueueMetadata.h"
#include "LgnTracing/LogStream.h"
#include "LgnTracing/MetricMetadata.h"
#include "LgnTracing/StringsMetadata.h"
#include "LgnTracing/LogMetadata.h"
#include "Dom/JsonValue.h"
#include "Dom/JsonObject.h"
#include "Serialization/JsonWriter.h"
#include "Serialization/JsonSerializer.h"
#include "MetricDependencies.h"
#include "ThreadDependencies.h"

void FormatContainerMetadata( TArray< TSharedPtr<FJsonValue> >& outUdts, const TArray< LgnTracing::UserDefinedType >& udts)
{
    for( const LgnTracing::UserDefinedType& udt: udts )
    {
        TSharedRef<FJsonObject> obj = MakeShareable(new FJsonObject);
        obj->SetStringField(TEXT("name"), udt.Name);
        obj->SetStringField(TEXT("size"), std::to_string(udt.Size).c_str());
        obj->SetBoolField(TEXT("is_reference"), udt.IsReference);

        TArray< TSharedPtr<FJsonValue> > members;
        for( const LgnTracing::UDTMember& member: udt.Members )
        {
            TSharedPtr<FJsonObject> memberObj = MakeShareable(new FJsonObject);
            memberObj->SetStringField(TEXT("name"), member.Name);
            memberObj->SetStringField(TEXT("type_name"), member.TypeName);
            memberObj->SetStringField(TEXT("offset"), std::to_string(member.Offset).c_str());
            memberObj->SetStringField(TEXT("size"), std::to_string(member.Size).c_str());
            memberObj->SetBoolField(TEXT("is_reference"), member.IsReference);
            members.Push(MakeShareable( new FJsonValueObject(memberObj)));
        }
        obj->SetArrayField(TEXT("members"), members);
        outUdts.Push(MakeShareable( new FJsonValueObject(obj)));
    }
}

template< typename DepQueue, typename StreamT >
FString FormatInsertStreamRequest( const StreamT& stream )
{
    using namespace LgnTracing;
    TSharedRef<FJsonObject> streamObj = MakeShareable(new FJsonObject);
    streamObj->SetStringField(TEXT("stream_id"), stream.GetStreamId().c_str());
    streamObj->SetStringField(TEXT("process_id"), stream.GetProcessId().c_str());

    TArray< TSharedPtr<FJsonValue> > depUdts;
    FormatContainerMetadata(depUdts, MakeQueueMetadata<DepQueue>()());
    streamObj->SetArrayField(TEXT("dependencies_metadata"), depUdts);

    TArray< TSharedPtr<FJsonValue> > objUdts;
    typedef typename StreamT::EventBlock EventBlock;
    typedef typename EventBlock::Queue EventQueue;
    FormatContainerMetadata(objUdts, MakeQueueMetadata<EventQueue>()());
    streamObj->SetArrayField(TEXT("objects_metadata"), objUdts);

    SetStringArrayField(*streamObj, TEXT("tags"), stream.GetTags());
    SetStringMapField(*streamObj, TEXT("properties"), stream.GetProperties());
        
    FString jsonText;
    TSharedRef< TJsonWriter<> > jsonWriter = TJsonWriterFactory<>::Create(&jsonText);
    if ( !FJsonSerializer::Serialize(streamObj, jsonWriter))
    {
        UE_LOG(LogLgnTelemetrySink, Error, TEXT("Error formatting udts as json"));
        return TEXT("");
    }
    jsonWriter->Close();
    return jsonText;
}

FString FormatInsertLogStreamRequest( const LgnTracing::LogStream& stream )
{
    return FormatInsertStreamRequest<LogDependenciesQueue>( stream );
}

FString FormatInsertMetricStreamRequest( const LgnTracing::MetricStream& stream )
{
    return FormatInsertStreamRequest<MetricDependenciesQueue>( stream );
}

FString FormatInsertThreadStreamRequest( const LgnTracing::ThreadStream& stream )
{
    return FormatInsertStreamRequest<ThreadDependenciesQueue>( stream );
}
