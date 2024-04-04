#pragma once
//
//  LgnTelemetrySink/JsonUtils.h
//
#include <string>
#include <vector>
#include <map>

class FJsonObject;

void SetStringArrayField(FJsonObject& obj, const TCHAR* name, const std::vector<std::wstring>& strings);
void SetStringMapField(FJsonObject& obj, const TCHAR* name, const std::map<std::wstring,std::wstring>& properties);
