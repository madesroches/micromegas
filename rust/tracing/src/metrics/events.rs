use crate::{prelude::*, property_set::PropertySet};
use micromegas_transit::prelude::*;

/// static metadata about metric events
#[derive(Debug)]
pub struct StaticMetricMetadata {
    pub lod: Verbosity,
    pub name: &'static str,
    pub unit: &'static str,
    pub target: &'static str,
    pub file: &'static str,
    pub line: u32,
}

/// Wire format reprensenting an instance of [StaticMetricMetadata]
#[derive(Debug, TransitReflect)]
pub struct MetricMetadataDependency {
    pub id: u64,
    pub name: *const u8,
    pub unit: *const u8,
    pub target: *const u8,
    pub file: *const u8,
    pub line: u32,
    pub lod: u32,
}

impl InProcSerialize for MetricMetadataDependency {}

/// Measure (int) with static metadata
/// Will be converted to a floating point value when processed by the analytics library
#[derive(Debug, TransitReflect)]
pub struct IntegerMetricEvent {
    pub desc: &'static StaticMetricMetadata,
    pub value: u64,
    pub time: i64,
}

impl InProcSerialize for IntegerMetricEvent {}

/// Measure (float) with static metadata
#[derive(Debug, TransitReflect)]
pub struct FloatMetricEvent {
    pub desc: &'static StaticMetricMetadata,
    pub value: f64,
    pub time: i64,
}

impl InProcSerialize for FloatMetricEvent {}

/// Measure (float) with a dynamic set of properties
#[derive(Debug, TransitReflect)]
pub struct TaggedFloatMetricEvent {
    pub desc: &'static StaticMetricMetadata,
    pub properties: &'static PropertySet,
    pub value: f64,
    pub time: i64,
}

impl InProcSerialize for TaggedFloatMetricEvent {}

/// Measure (int) with a dynamic set of properties
#[derive(Debug, TransitReflect)]
pub struct TaggedIntegerMetricEvent {
    pub desc: &'static StaticMetricMetadata,
    pub properties: &'static PropertySet,
    pub value: u64,
    pub time: i64,
}

impl InProcSerialize for TaggedIntegerMetricEvent {}
