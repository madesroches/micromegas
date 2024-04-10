//
//  MicromegasTelemetrySink/CborUtils.cpp
//
#include "CborUtils.h"

void encode_utf8_string(jsoncons::cbor::cbor_bytes_encoder& encoder, const TCHAR* str)
{
	using string_view_type = jsoncons::cbor::cbor_bytes_encoder::string_view_type;
	FTCHARToUTF8 UTF8String(str);
	encoder.string_value(string_view_type(UTF8String.Get(), UTF8String.Length()));
}
