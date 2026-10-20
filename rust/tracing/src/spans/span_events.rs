use crate::property_set::PropertySetId;
use crate::string_id::{DynStringId, StringId};
use micromegas_transit::prelude::*;

use super::{SpanLocation, SpanMetadata};

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
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for BeginAsyncSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for EndAsyncSpanEvent {}
#[derive(Debug, TransitReflect)]
pub struct BeginAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for BeginAsyncNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: StringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for EndAsyncNamedSpanEvent {}

//
// events whose name is a block-scoped dynamic string record
//
#[derive(Debug, TransitReflect)]
pub struct BeginThreadDynNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: DynStringId,
    pub time: i64,
}

impl InProcSerialize for BeginThreadDynNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndThreadDynNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: DynStringId,
    pub time: i64,
}

impl InProcSerialize for EndThreadDynNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct BeginAsyncDynNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: DynStringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for BeginAsyncDynNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct EndAsyncDynNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: DynStringId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for EndAsyncDynNamedSpanEvent {}

//
// end events that carry properties
//
#[derive(Debug, TransitReflect)]
pub struct TaggedEndThreadSpanEvent {
    pub thread_span_desc: &'static SpanMetadata,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}

impl InProcSerialize for TaggedEndThreadSpanEvent {}

/// The name is always a block-scoped string record, whether it was `'static` or built at runtime.
#[derive(Debug, TransitReflect)]
pub struct TaggedEndThreadNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: DynStringId,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}

impl InProcSerialize for TaggedEndThreadNamedSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct TaggedEndAsyncSpanEvent {
    pub span_desc: &'static SpanMetadata,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for TaggedEndAsyncSpanEvent {}

#[derive(Debug, TransitReflect)]
pub struct TaggedEndAsyncNamedSpanEvent {
    pub span_location: &'static SpanLocation,
    pub name: DynStringId,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub span_id: u64,
    pub parent_span_id: u64, // parent span at future creation time
    pub depth: u32,          // nesting depth in async call hierarchy
    pub time: i64,
}

impl InProcSerialize for TaggedEndAsyncNamedSpanEvent {}
