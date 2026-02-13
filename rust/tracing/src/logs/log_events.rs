use crate::{property_set::PropertySet, static_string_ref::StaticStringRef, string_id::StringId};
use micromegas_transit::{
    DynString, UserDefinedType, prelude::*, read_advance_string, read_consume_pod,
};
use std::sync::Arc;

use super::LogMetadata;

// Ensure we're on a 64-bit platform since we store pointers as u64
const _: () = assert!(std::mem::size_of::<usize>() == 8);

#[derive(Debug, TransitReflect)]
pub struct LogStaticStrEvent {
    pub desc: &'static LogMetadata<'static>,
    pub time: i64,
}

impl InProcSerialize for LogStaticStrEvent {}

#[derive(Debug)]
pub struct LogStringEvent {
    pub desc: &'static LogMetadata<'static>,
    pub time: i64,
    pub msg: DynString,
}

impl InProcSerialize for LogStringEvent {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<usize>() as u32 //desc reference
                + std::mem::size_of::<i64>() as u32 //time
                + self.msg.get_value_size().unwrap(), //dyn string
        )
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let desc_id = self.desc as *const _ as usize;
        write_any(buffer, &desc_id);
        write_any(buffer, &self.time);
        self.msg.write_value(buffer);
    }

    unsafe fn read_value(mut window: &[u8]) -> Self {
        // it does no good to parse this object when looking for dependencies, we should skip this code
        let desc_id: usize = read_consume_pod(&mut window);
        let desc = unsafe { &*(desc_id as *const LogMetadata) };
        let time: i64 = read_consume_pod(&mut window);
        let msg = DynString(read_advance_string(&mut window).unwrap());
        assert_eq!(window.len(), 0);
        Self { desc, time, msg }
    }
}

//todo: change this interface to make clear that there are two serialization
// strategies: pod and custom
impl Reflect for LogStringEvent {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("LogStringEventV2".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

#[derive(Debug, TransitReflect)]
pub struct LogStaticStrInteropEvent {
    pub time: i64,
    pub level: u32,
    pub target: StringId,
    pub msg: StringId,
}

impl InProcSerialize for LogStaticStrInteropEvent {}

#[derive(Debug)]
pub struct LogStringInteropEvent {
    pub time: i64,
    pub level: u8,
    pub target: StaticStringRef, //for unreal compatibility
    pub msg: DynString,
}

impl InProcSerialize for LogStringInteropEvent {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<i64>() as u32 //time
                + std::mem::size_of::<u8>() as u32 //level
                + std::mem::size_of::<StringId>() as u32 //target
                + self.msg.get_value_size().unwrap(), //message
        )
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        write_any(buffer, &self.time);
        write_any(buffer, &self.level);
        write_any(buffer, &self.target);
        self.msg.write_value(buffer);
    }

    unsafe fn read_value(mut window: &[u8]) -> Self {
        let time: i64 = read_consume_pod(&mut window);
        let level: u8 = read_consume_pod(&mut window);
        let target: StaticStringRef = read_consume_pod(&mut window);
        let msg = DynString(read_advance_string(&mut window).unwrap());
        Self {
            time,
            level,
            target,
            msg,
        }
    }
}

//todo: change this interface to make clear that there are two serialization
// strategies: pod and custom
impl Reflect for LogStringInteropEvent {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("LogStringInteropEventV3".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![StaticStringRef::reflect()],
        }
    }
}

#[derive(Debug, TransitReflect)]
pub struct LogMetadataRecord {
    pub id: u64,
    pub fmt_str: *const u8,
    pub target: *const u8,
    pub module_path: *const u8,
    pub file: *const u8,
    pub line: u32,
    pub level: u32,
}

impl InProcSerialize for LogMetadataRecord {}

#[derive(Debug)]
pub struct TaggedLogString {
    pub desc: &'static LogMetadata<'static>,
    pub properties: &'static PropertySet,
    pub time: i64,
    pub msg: DynString,
}

impl Reflect for TaggedLogString {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("TaggedLogString".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for TaggedLogString {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<u64>() as u32 // desc ptr
            + std::mem::size_of::<u64>() as u32 // properties ptr
            + std::mem::size_of::<i64>() as u32 // time
                + self.msg.get_value_size().unwrap(), // message
        )
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        write_any(buffer, &self.desc);
        write_any(buffer, &self.properties);
        write_any(buffer, &self.time);
        self.msg.write_value(buffer);
    }

    #[allow(unknown_lints)]
    #[allow(integer_to_ptr_transmutes)] // TODO: Fix pointer provenance properly (see tasks/pointer-provenance.md)
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let desc_id: u64 = read_consume_pod(&mut window);
        let properties_id: u64 = read_consume_pod(&mut window);
        let time: i64 = read_consume_pod(&mut window);
        let msg = DynString(read_advance_string(&mut window).unwrap());
        Self {
            desc: unsafe { std::mem::transmute::<u64, &LogMetadata<'static>>(desc_id) },
            properties: unsafe { std::mem::transmute::<u64, &PropertySet>(properties_id) },
            time,
            msg,
        }
    }
}
