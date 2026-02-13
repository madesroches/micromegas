use crate::prelude::*;

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
