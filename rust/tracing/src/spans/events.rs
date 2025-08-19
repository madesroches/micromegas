use crate::{prelude::*, string_id::StringId};
use micromegas_transit::prelude::*;

#[derive(Debug)]
pub struct SpanLocation {
    pub lod: Verbosity,
    pub target: &'static str,
    pub module_path: &'static str,
    pub file: &'static str,
    pub line: u32,
}

// SpanLocationRecord is the serialized version of SpanLocation
#[derive(Debug, TransitReflect)]
pub struct SpanLocationRecord {
    pub id: u64,
    pub target: *const u8,
    pub module_path: *const u8,
    pub file: *const u8,
    pub line: u32,
    pub lod: u32,
}

impl InProcSerialize for SpanLocationRecord {}

#[derive(Debug)]
pub struct SpanMetadata {
    pub name: &'static str,
    pub location: SpanLocation,
}

// SpanRecord is the serialized version of SpanMetadata
#[derive(Debug, TransitReflect)]
pub struct SpanRecord {
    pub id: u64,
    pub name: *const u8,
    pub target: *const u8,
    pub module_path: *const u8,
    pub file: *const u8,
    pub line: u32,
    pub lod: u32,
}

impl InProcSerialize for SpanRecord {}

//
// sync events
//
#[derive(Debug, TransitReflect)]
pub struct BeginThreadSpanEvent {
    pub thread_span_desc: &'static SpanMetadata,
    pub time: i64,
}

impl InProcSerialize for BeginThreadSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndThreadSpanEvent {
    pub thread_span_desc: &'static SpanMetadata,
    pub time: i64,
}

impl InProcSerialize for EndThreadSpanEvent {}
#[derive(Debug, TransitReflect)]
pub struct BeginThreadNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: StringId,
    pub time: i64,
}

impl InProcSerialize for BeginThreadNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndThreadNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: StringId,
    pub time: i64,
}

impl InProcSerialize for EndThreadNamedSpanEvent {}

//
// async events
//
#[derive(Debug, TransitReflect)]
pub struct BeginAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span when first polled
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for BeginAsyncSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span when completed
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for EndAsyncSpanEvent {}
#[derive(Debug, TransitReflect)]
pub struct BeginAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span when first polled
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for BeginAsyncNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span when completed
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for EndAsyncNamedSpanEvent {}
