use crate::{string_id::StringId, Level, LevelFilter};
use micromegas_transit::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
pub struct LogMetadata {
    pub level: Level,
    pub level_filter: AtomicU32,
    pub fmt_str: &'static str,
    pub target: &'static str,
    pub module_path: &'static str,
    pub file: &'static str,
    pub line: u32,
}

pub const FILTER_LEVEL_UNSET_VALUE: u32 = 0xF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterState {
    /// The filter needs to be updated.
    Outdated,
    /// The filter is up to date but no filter is set.
    NotSet,
    /// The filter is up to date a filter level is set.
    Set(LevelFilter),
}

impl LogMetadata {
    /// This is a way to efficiency implement finer grade filtering by amortizing its
    /// cost. An atomic is used to store a level filter and a 16 bit generation.
    /// Allowing a config update to be applied to the level filter multiple times during
    /// the lifetime of the process.
    ///
    /// ```ignore
    /// const GENERATION: u16 = 1;
    /// let level_filter = metadata.level_filter(GENERATION).unwrap_or_else(|| {
    ///     let level_filter = self.level_filter(metadata.target);
    ///     metadata.set_level_filter(level_filter, GENERATION);
    ///     level_filter
    /// });
    /// if metadata.level <= level_filter {
    ///     ...
    /// }
    /// ```
    ///
    pub fn level_filter(&self, generation: u16) -> FilterState {
        let level_filter = self.level_filter.load(Ordering::Relaxed);
        if generation > ((level_filter >> 16) as u16) {
            FilterState::Outdated
        } else {
            LevelFilter::from_u32(level_filter & FILTER_LEVEL_UNSET_VALUE)
                .map_or(FilterState::NotSet, |level_filter| {
                    FilterState::Set(level_filter)
                })
        }
    }

    /// Sets the level filter if the generation is greater than the current generation.
    ///
    pub fn set_level_filter(&self, generation: u16, level_filter: Option<LevelFilter>) {
        let new = level_filter.map_or(FILTER_LEVEL_UNSET_VALUE, |filter_level| filter_level as u32)
            | u32::from(generation) << 16;
        let mut current = self.level_filter.load(Ordering::Relaxed);
        if generation <= (current >> 16) as u16 {
            // value was updated form another thread with a newer generation
            return;
        }
        loop {
            match self.level_filter.compare_exchange(
                current,
                new,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return;
                }
                Err(x) => {
                    if generation <= (x >> 16) as u16 {
                        return;
                    }
                    current = x;
                }
            };
        }
    }
}

#[derive(Debug, TransitReflect)]
pub struct LogStaticStrEvent {
    pub desc: &'static LogMetadata,
    pub time: i64,
}

impl InProcSerialize for LogStaticStrEvent {}

#[derive(Debug)]
pub struct LogStringEvent {
    pub desc: &'static LogMetadata,
    pub time: i64,
    pub dyn_str: DynString,
}

impl InProcSerialize for LogStringEvent {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<usize>() as u32 //desc reference
                + std::mem::size_of::<i64>() as u32 //time
                + self.dyn_str.get_value_size().unwrap(), //dyn string
        )
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let desc_id = self.desc as *const _ as usize;
        write_any(buffer, &desc_id);
        write_any(buffer, &self.time);
        self.dyn_str.write_value(buffer);
    }

    unsafe fn read_value(ptr: *const u8, value_size: Option<u32>) -> Self {
        let desc_id = read_any::<usize>(ptr);
        let desc = &*(desc_id as *const LogMetadata);
        let time_offset = std::mem::size_of::<usize>();
        let time = read_any::<i64>(ptr.add(time_offset));
        let buffer_size = value_size.unwrap();
        let string_offset = std::mem::size_of::<i64>() + time_offset;
        let string_ptr = ptr.add(string_offset);
        let msg = <DynString as InProcSerialize>::read_value(
            string_ptr,
            Some(buffer_size - string_offset as u32),
        );
        Self {
            desc,
            time,
            dyn_str: msg,
        }
    }
}

//todo: change this interface to make clear that there are two serialization
// strategies: pod and custom
impl Reflect for LogStringEvent {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("LogStringEvent"),
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
    pub level: u32,
    pub target: StringId,
    pub msg: DynString,
}

impl InProcSerialize for LogStringInteropEvent {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<i64>() as u32 //time
                + std::mem::size_of::<u32>() as u32 //level
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

    unsafe fn read_value(ptr: *const u8, value_size: Option<u32>) -> Self {
        let time = read_any::<i64>(ptr);
        let level_offset = std::mem::size_of::<i64>();
        let level = read_any::<u32>(ptr.add(level_offset));
        let target_offset = level_offset + std::mem::size_of::<u32>();
        let target = read_any::<StringId>(ptr.add(target_offset));
        let buffer_size = value_size.unwrap();
        let msg_offset = target_offset + std::mem::size_of::<StringId>();
        let msg_ptr = ptr.add(msg_offset);
        let msg = <DynString as InProcSerialize>::read_value(
            msg_ptr,
            Some(buffer_size - msg_offset as u32),
        );
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
        UserDefinedType {
            name: String::from("LogStringInteropEventV2"),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
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

#[cfg(test)]
mod test {
    use std::thread;

    use crate::{
        logs::{FilterState, LogMetadata, FILTER_LEVEL_UNSET_VALUE},
        Level, LevelFilter,
    };

    #[test]
    fn test_filter_levels() {
        static METADATA: LogMetadata = LogMetadata {
            level: Level::Trace,
            level_filter: std::sync::atomic::AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
            fmt_str: "$crate::__first_arg!($($arg)+)",
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        assert_eq!(METADATA.level_filter(1), FilterState::Outdated);
        METADATA.set_level_filter(1, Some(LevelFilter::Trace));
        assert_eq!(
            METADATA.level_filter(1),
            FilterState::Set(LevelFilter::Trace)
        );
        METADATA.set_level_filter(1, Some(LevelFilter::Debug));
        assert_eq!(
            METADATA.level_filter(1),
            FilterState::Set(LevelFilter::Trace)
        );
        METADATA.set_level_filter(1, None);
        assert_eq!(
            METADATA.level_filter(1),
            FilterState::Set(LevelFilter::Trace)
        );
        METADATA.set_level_filter(2, Some(LevelFilter::Debug));
        assert_eq!(
            METADATA.level_filter(1),
            FilterState::Set(LevelFilter::Debug)
        );
        assert_eq!(
            METADATA.level_filter(2),
            FilterState::Set(LevelFilter::Debug)
        );
        METADATA.set_level_filter(1, Some(LevelFilter::Info));
        assert_eq!(
            METADATA.level_filter(2),
            FilterState::Set(LevelFilter::Debug)
        );
        assert_eq!(METADATA.level_filter(3), FilterState::Outdated);
        METADATA.set_level_filter(3, None);
        assert_eq!(METADATA.level_filter(3), FilterState::NotSet);
        METADATA.set_level_filter(3, Some(LevelFilter::Warn));
        assert_eq!(METADATA.level_filter(3), FilterState::NotSet);

        let mut threads = Vec::new();
        for _ in 0..1 {
            threads.push(thread::spawn(move || {
                for i in 0..1024 {
                    let filter = match i % 6 {
                        0 => LevelFilter::Off,
                        1 => LevelFilter::Error,
                        2 => LevelFilter::Warn,
                        3 => LevelFilter::Info,
                        4 => LevelFilter::Debug,
                        5 => LevelFilter::Trace,
                        _ => unreachable!(),
                    };

                    METADATA.set_level_filter(i, Some(filter));
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(
            METADATA.level_filter(1023),
            FilterState::Set(LevelFilter::Info)
        );
        assert_eq!(METADATA.level_filter(1024), FilterState::Outdated);
    }
}
