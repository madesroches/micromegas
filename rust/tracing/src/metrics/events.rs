use crate::prelude::*;
use micromegas_transit::prelude::*;

#[derive(Debug)]
pub struct MetricMetadata {
    pub lod: Verbosity,
    pub name: &'static str,
    pub unit: &'static str,
    pub target: &'static str,
    pub file: &'static str,
    pub line: u32,
}

#[derive(Debug, TransitReflect)]
pub struct IntegerMetricEvent {
    pub desc: &'static MetricMetadata,
    pub value: u64,
    pub time: i64,
}

impl InProcSerialize for IntegerMetricEvent {}

#[derive(Debug, TransitReflect)]
pub struct FloatMetricEvent {
    pub desc: &'static MetricMetadata,
    pub value: f64,
    pub time: i64,
}

impl InProcSerialize for FloatMetricEvent {}
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
