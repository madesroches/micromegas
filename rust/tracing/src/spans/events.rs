use crate::prelude::*;

#[derive(Debug)]
pub struct SpanLocation {
    pub lod: Verbosity,
    pub target: &'static str,
    pub module_path: &'static str,
    pub file: &'static str,
    pub line: u32,
}

#[derive(Debug)]
pub struct SpanMetadata {
    pub name: &'static str,
    pub location: SpanLocation,
}
