use crate::levels::{Level, LevelFilter};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
pub struct LogMetadata<'a> {
    pub level: Level,
    pub level_filter: AtomicU32,
    pub fmt_str: &'a str,
    pub target: &'a str,
    pub module_path: &'a str,
    pub file: &'a str,
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

impl LogMetadata<'_> {
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
            | (u32::from(generation) << 16);
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
