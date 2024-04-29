#pragma once
//
//  MicromegasTelemetrySink/CborUtils.h
//
#include "jsoncons/json.hpp"
#include "jsoncons_ext/cbor/cbor.hpp"
#include "HAL/Platform.h"

void encode_utf8_string(jsoncons::cbor::cbor_bytes_encoder& encoder, const TCHAR* str);
