#pragma once
//
//  LgnTelemetrySink/JsonUtils.cpp
//
#include "JsonUtils.h"
#include "Dom/JsonValue.h"
#include "Dom/JsonObject.h"

void SetStringArrayField(FJsonObject& obj, const TCHAR* name, const std::vector<std::wstring>& strings)
{
    TArray< TSharedPtr<FJsonValue> > values;
    for( const std::wstring& str: strings )
    {
        values.Push(MakeShareable(new FJsonValueString(str.c_str())));
    }
    obj.SetArrayField( name, values );
}

void SetStringMapField(FJsonObject& obj, const TCHAR* name, const std::map<std::wstring,std::wstring>& properties)
{
    TSharedRef<FJsonObject> map = MakeShareable(new FJsonObject);
    for( const std::pair<std::wstring,std::wstring>& kv: properties)
    {
        map->SetStringField( kv.first.c_str(), kv.second.c_str() );
    }
    obj.SetObjectField( name, map );
 }
