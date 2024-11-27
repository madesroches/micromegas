//! Verbosity management
use std::{
    cmp, fmt,
    hash::Hash,
    str::FromStr,
    sync::atomic::{self, AtomicU32},
};

/// An enum representing the verbosity levels for logging.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum Level {
    /// The "fatal" level.
    ///
    /// Crashes, panics.
    Fatal = 1,
    /// The "error" level.
    ///
    /// Designates very serious errors.
    Error,
    /// The "warn" level.
    ///
    /// Designates hazardous situations.
    Warn,
    /// The "info" level.
    ///
    /// Designates useful information.
    Info,
    /// The "debug" level.
    ///
    /// Designates lower priority information.
    Debug,
    /// The "trace" level.
    ///
    /// Designates very low priority, often extremely verbose, information.
    Trace,
}

impl Level {
    pub fn from_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Fatal),
            2 => Some(Self::Error),
            3 => Some(Self::Warn),
            4 => Some(Self::Info),
            5 => Some(Self::Debug),
            6 => Some(Self::Trace),
            _ => None,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fatal" => Some(Self::Fatal),
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }
}

/// An enum representing the available verbosity level filters of the logger.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum LevelFilter {
    /// A level lower than all log levels.
    Off,
    /// Corresponds to the `Fatal` log level.
    Fatal,
    /// Corresponds to the `Error` log level.
    Error,
    /// Corresponds to the `Warn` log level.
    Warn,
    /// Corresponds to the `Info` log level.
    Info,
    /// Corresponds to the `Debug` log level.
    Debug,
    /// Corresponds to the `Trace` log level.
    Trace,
}

impl PartialEq<LevelFilter> for Level {
    #[inline(always)]
    fn eq(&self, other: &LevelFilter) -> bool {
        *self as u32 == *other as u32
    }
}

impl PartialOrd for Level {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &Self) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Self) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Self) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Self) -> bool {
        *self as u32 >= *other as u32
    }
}

impl PartialOrd<LevelFilter> for Level {
    #[inline(always)]
    fn partial_cmp(&self, other: &LevelFilter) -> Option<cmp::Ordering> {
        Some((*self as u32).cmp(&(*other as u32)))
    }

    #[inline(always)]
    fn lt(&self, other: &LevelFilter) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &LevelFilter) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &LevelFilter) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &LevelFilter) -> bool {
        *self as u32 >= *other as u32
    }
}

impl Ord for Level {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (*self as u32).cmp(&(*other as u32))
    }
}

fn ok_or<T, E>(t: Option<T>, e: E) -> Result<T, E> {
    match t {
        Some(t) => Ok(t),
        None => Err(e),
    }
}

pub struct ParseLevelError(());

impl FromStr for Level {
    type Err = ParseLevelError;
    fn from_str(level: &str) -> Result<Self, Self::Err> {
        ok_or(
            LEVEL_NAMES
                .iter()
                .position(|&name| str::eq_ignore_ascii_case(name, level))
                .into_iter()
                .filter(|&idx| idx != 0)
                .map(|idx| Self::from_u32(idx as u32).unwrap())
                .next(),
            ParseLevelError(()),
        )
    }
}

impl fmt::Display for Level {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.pad(self.as_str())
    }
}

impl Level {
    pub(crate) fn from_u32(u: u32) -> Option<Self> {
        match u {
            1 => Some(Self::Fatal),
            2 => Some(Self::Error),
            3 => Some(Self::Warn),
            4 => Some(Self::Info),
            5 => Some(Self::Debug),
            6 => Some(Self::Trace),
            _ => None,
        }
    }
    /// Returns the most verbose logging level.
    #[inline(always)]
    pub fn max() -> Self {
        Self::Trace
    }

    /// Converts the `Level` to the equivalent `LevelFilter`.
    #[inline(always)]
    pub fn to_level_filter(self) -> LevelFilter {
        LevelFilter::from_u32(self as u32).unwrap()
    }

    /// Returns the string representation of the `Level`.
    ///
    /// This returns the same string as the `fmt::Display` implementation.
    pub fn as_str(self) -> &'static str {
        LEVEL_NAMES[self as usize]
    }

    /// Iterate through all supported logging levels.
    ///
    /// The order of iteration is from more severe to less severe log messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    ///
    /// let mut levels = Level::iter();
    ///
    /// assert_eq!(Some(Level::Fatal), levels.next());
    /// assert_eq!(Some(Level::Trace), levels.last());
    /// ```
    pub fn iter() -> impl Iterator<Item = Self> {
        (1..7).map(|i| Self::from_u32(i).unwrap())
    }
}

impl PartialEq<Level> for LevelFilter {
    #[inline(always)]
    fn eq(&self, other: &Level) -> bool {
        other.eq(self)
    }
}

impl PartialOrd for LevelFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &Self) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Self) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Self) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Self) -> bool {
        *self as u32 >= *other as u32
    }
}

impl PartialOrd<Level> for LevelFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Level) -> Option<cmp::Ordering> {
        Some((*self as u32).cmp(&(*other as u32)))
    }

    #[inline(always)]
    fn lt(&self, other: &Level) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Level) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Level) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Level) -> bool {
        *self as u32 >= *other as u32
    }
}

impl Ord for LevelFilter {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (*self as u32).cmp(&(*other as u32))
    }
}

impl FromStr for LevelFilter {
    type Err = ParseLevelError;
    fn from_str(level: &str) -> Result<Self, Self::Err> {
        ok_or(
            LEVEL_NAMES
                .iter()
                .position(|&name| str::eq_ignore_ascii_case(name, level))
                .map(|p| Self::from_u32(p as u32).unwrap()),
            ParseLevelError(()),
        )
    }
}

impl fmt::Display for LevelFilter {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.pad(self.as_str())
    }
}

impl LevelFilter {
    pub(crate) fn from_u32(u: u32) -> Option<Self> {
        match u {
            0 => Some(Self::Off),
            1 => Some(Self::Fatal),
            2 => Some(Self::Error),
            3 => Some(Self::Warn),
            4 => Some(Self::Info),
            5 => Some(Self::Debug),
            6 => Some(Self::Trace),
            _ => None,
        }
    }

    /// Returns the most verbose logging level filter.
    #[inline(always)]
    pub fn max() -> Self {
        Self::Trace
    }

    /// Converts `self` to the equivalent `Level`.
    ///
    /// Returns `None` if `self` is `LevelFilter::Off`.
    #[inline(always)]
    pub fn to_level(self) -> Option<Level> {
        Level::from_u32(self as u32)
    }

    /// Returns the string representation of the `LevelFilter`.
    ///
    /// This returns the same string as the `fmt::Display` implementation.
    pub fn as_str(self) -> &'static str {
        LEVEL_NAMES[self as usize]
    }

    /// Iterate through all supported filtering levels.
    ///
    /// The order of iteration is from less to more verbose filtering.
    ///
    /// # Examples
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    ///
    /// let mut levels = LevelFilter::iter();
    ///
    /// assert_eq!(Some(LevelFilter::Off), levels.next());
    /// assert_eq!(Some(LevelFilter::Trace), levels.last());
    /// ```
    pub fn iter() -> impl Iterator<Item = Self> {
        (0..7).map(|i| Self::from_u32(i).unwrap())
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
/// An enum representing the level of verbosity for `metrics`/`thread_spans`/`spans`.
pub enum Verbosity {
    /// The "min" level.
    ///
    /// Designates vey low details events, meaning overall lower frequency.
    Min = 1,
    /// The "med" level.
    ///
    /// Designates medium level level of details, meaning overall medium frequency
    Med,
    /// The "Max" level.
    ///
    /// Designates very high frequency events.
    Max,
}

/// An enum representing the available verbosity level filters of the logger.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum LodFilter {
    /// A level lower than all log levels.
    Off,
    /// Corresponds to the `Min` log level.
    Min,
    /// Corresponds to the `Med` log level.
    Med,
    /// Corresponds to the `Max` log level.
    Max,
}

impl PartialEq<LodFilter> for Verbosity {
    #[inline(always)]
    fn eq(&self, other: &LodFilter) -> bool {
        *self as u32 == *other as u32
    }
}

impl PartialOrd for Verbosity {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &Self) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Self) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Self) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Self) -> bool {
        *self as u32 >= *other as u32
    }
}

impl PartialOrd<LodFilter> for Verbosity {
    #[inline(always)]
    fn partial_cmp(&self, other: &LodFilter) -> Option<cmp::Ordering> {
        Some((*self as u32).cmp(&(*other as u32)))
    }

    #[inline(always)]
    fn lt(&self, other: &LodFilter) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &LodFilter) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &LodFilter) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &LodFilter) -> bool {
        *self as u32 >= *other as u32
    }
}

impl Ord for Verbosity {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (*self as u32).cmp(&(*other as u32))
    }
}

impl FromStr for Verbosity {
    type Err = ParseLevelError;
    fn from_str(level: &str) -> Result<Self, Self::Err> {
        ok_or(
            LOD_NAMES
                .iter()
                .position(|&name| str::eq_ignore_ascii_case(name, level))
                .into_iter()
                .filter(|&idx| idx != 0)
                .map(|idx| Self::from_usize(idx).unwrap())
                .next(),
            ParseLevelError(()),
        )
    }
}

impl fmt::Display for Verbosity {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.pad(self.as_str())
    }
}

impl Verbosity {
    fn from_usize(u: usize) -> Option<Self> {
        match u {
            1 => Some(Self::Min),
            2 => Some(Self::Med),
            3 => Some(Self::Max),
            _ => None,
        }
    }

    fn from_u32(u: u32) -> Option<Self> {
        match u {
            1 => Some(Self::Min),
            2 => Some(Self::Med),
            3 => Some(Self::Max),
            _ => None,
        }
    }

    /// Returns the most verbose logging level.
    #[inline(always)]
    pub fn max() -> Self {
        Self::Max
    }

    /// Converts the `Lod` to the equivalent `LodFilter`.
    #[inline(always)]
    pub fn to_level_filter(self) -> LodFilter {
        LodFilter::from_u32(self as u32).unwrap()
    }

    /// Returns the string representation of the `Lod`.
    ///
    /// This returns the same string as the `fmt::Display` implementation.
    pub fn as_str(self) -> &'static str {
        LOD_NAMES[self as usize]
    }

    /// Iterate through all supported logging levels.
    ///
    /// The order of iteration is from more severe to less severe log messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    ///
    /// let mut lods = Verbosity::iter();
    ///
    /// assert_eq!(Some(Verbosity::Min), lods.next());
    /// assert_eq!(Some(Verbosity::Max), lods.last());
    /// ```
    pub fn iter() -> impl Iterator<Item = Self> {
        (1..4).map(|i| Self::from_usize(i).unwrap())
    }
}

impl PartialEq<Verbosity> for LodFilter {
    #[inline(always)]
    fn eq(&self, other: &Verbosity) -> bool {
        other.eq(self)
    }
}

impl PartialOrd for LodFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &Self) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Self) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Self) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Self) -> bool {
        *self as u32 >= *other as u32
    }
}

impl PartialOrd<Verbosity> for LodFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Verbosity) -> Option<cmp::Ordering> {
        Some((*self as u32).cmp(&(*other as u32)))
    }

    #[inline(always)]
    fn lt(&self, other: &Verbosity) -> bool {
        (*self as u32) < *other as u32
    }

    #[inline(always)]
    fn le(&self, other: &Verbosity) -> bool {
        *self as u32 <= *other as u32
    }

    #[inline(always)]
    fn gt(&self, other: &Verbosity) -> bool {
        *self as u32 > *other as u32
    }

    #[inline(always)]
    fn ge(&self, other: &Verbosity) -> bool {
        *self as u32 >= *other as u32
    }
}

impl Ord for LodFilter {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (*self as u32).cmp(&(*other as u32))
    }
}

impl FromStr for LodFilter {
    type Err = ParseLevelError;
    fn from_str(level: &str) -> Result<Self, Self::Err> {
        ok_or(
            LOD_NAMES
                .iter()
                .position(|&name| str::eq_ignore_ascii_case(name, level))
                .map(|p| Self::from_usize(p).unwrap()),
            ParseLevelError(()),
        )
    }
}

impl fmt::Display for LodFilter {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.pad(self.as_str())
    }
}

impl LodFilter {
    fn from_usize(u: usize) -> Option<Self> {
        match u {
            0 => Some(Self::Off),
            1 => Some(Self::Min),
            2 => Some(Self::Med),
            3 => Some(Self::Max),
            _ => None,
        }
    }

    fn from_u32(u: u32) -> Option<Self> {
        match u {
            0 => Some(Self::Off),
            1 => Some(Self::Min),
            2 => Some(Self::Med),
            3 => Some(Self::Max),
            _ => None,
        }
    }

    /// Returns the most verbose logging level filter.
    #[inline(always)]
    pub fn max() -> Self {
        Self::Max
    }

    /// Converts `self` to the equivalent `Lod`.
    ///
    /// Returns `None` if `self` is `LodFilter::Off`.
    #[inline(always)]
    pub fn to_level(self) -> Option<Verbosity> {
        Verbosity::from_u32(self as u32)
    }

    /// Returns the string representation of the `LodFilter`.
    ///
    /// This returns the same string as the `fmt::Display` implementation.
    pub fn as_str(self) -> &'static str {
        LOD_NAMES[self as usize]
    }

    /// Iterate through all supported filtering levels.
    ///
    /// The order of iteration is from less to more verbose filtering.
    ///
    /// # Examples
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    ///
    /// let mut lod_filters = LodFilter::iter();
    ///
    /// assert_eq!(Some(LodFilter::Off), lod_filters.next());
    /// assert_eq!(Some(LodFilter::Max), lod_filters.last());
    /// ```
    pub fn iter() -> impl Iterator<Item = Self> {
        (0..4).map(|i| Self::from_usize(i).unwrap())
    }
}

static MAX_LEVEL_FILTER: AtomicU32 = AtomicU32::new(0);
static MAX_LOD_FILTER: AtomicU32 = AtomicU32::new(0);

static LEVEL_NAMES: [&str; 7] = ["OFF", "FATAL", "ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
static LOD_NAMES: [&str; 4] = ["OFF", "LOW", "MED", "HIGH"];

/// Sets the global maximum log level.
///
/// Generally, this should only be called by the active logging implementation.
///
/// Note that `Trace` is the maximum level, because it provides the maximum amount of detail in the emitted logs.
#[inline(always)]
pub fn set_max_level(level: LevelFilter) {
    MAX_LEVEL_FILTER.store(level as u32, atomic::Ordering::Relaxed);
}

/// Returns the current maximum log level.
#[inline(always)]
pub fn max_level() -> LevelFilter {
    // Since `LevelFilter` is `repr(u32)`,
    // this transmute is sound if and only if `MAX_LOG_LEVEL_FILTER`
    // is set to a u32 that is a valid discriminant for `LevelFilter`.
    // Since `MAX_LOG_LEVEL_FILTER` is private, the only time it's set
    // is by `set_max_level` above, i.e. by casting a `LevelFilter` to `u32`.
    // So any u32 stored in `MAX_LOG_LEVEL_FILTER` is a valid discriminant.
    unsafe { std::mem::transmute(MAX_LEVEL_FILTER.load(atomic::Ordering::Relaxed)) }
}

/// Sets the global maximum log level.
#[inline(always)]
pub fn set_max_lod(level: LodFilter) {
    MAX_LOD_FILTER.store(level as u32, atomic::Ordering::Relaxed);
}

/// Returns the current maximum log level.
#[inline(always)]
pub fn max_lod() -> LodFilter {
    // See comment above
    unsafe { std::mem::transmute(MAX_LOD_FILTER.load(atomic::Ordering::Relaxed)) }
}

/// The statically resolved maximum log level.
///
/// See the crate level documentation for information on how to configure this.
///
/// This value is checked by the log macros, but not by the `Log`ger returned by
/// the [`logger`] function. Code that manually calls functions on that value
/// should compare the level against this value.
///
/// [`logger`]: fn.logger.html
pub const STATIC_MAX_LEVEL: LevelFilter = MAX_LEVEL_INNER;

cfg_if::cfg_if! {
    if #[cfg(all(not(debug_assertions), feature = "release_max_level_off"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Off;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_level_error"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Error;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_level_warn"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Warn;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_level_info"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Info;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_level_debug"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Debug;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_level_trace"))] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Trace;
    } else if #[cfg(feature = "max_level_off")] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Off;
    } else if #[cfg(feature = "max_level_error")] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Error;
    } else if #[cfg(feature = "max_level_warn")] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Warn;
    } else if #[cfg(feature = "max_level_info")] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Info;
    } else if #[cfg(feature = "max_level_debug")] {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Debug;
    } else {
        const MAX_LEVEL_INNER: LevelFilter = LevelFilter::Trace;
    }
}

/// The statically resolved maximum metrics/spans lod.
///
/// See the crate level documentation for information on how to configure this.
///
/// This value is checked by the log macros, but not by the `Log`ger returned by
/// the [`logger`] function. Code that manually calls functions on that value
/// should compare the level against this value.
///
/// [`logger`]: fn.logger.html
pub const STATIC_MAX_LOD: LodFilter = MAX_LOD_INNER;

cfg_if::cfg_if! {
    if #[cfg(all(not(debug_assertions), feature = "release_max_lod_off"))] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Off;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_lod_min"))] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Min;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_lod_med"))] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Med;
    } else if #[cfg(all(not(debug_assertions), feature = "release_max_lod_max"))] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Max;
    } else if #[cfg(feature = "max_lod_off")] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Off;
    } else if #[cfg(feature = "max_lod_min")] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Min;
    } else if #[cfg(feature = "max_lod_med")] {
        const MAX_LOD_INNER: LodFilter = LodFilter::Med;
    } else {
        const MAX_LOD_INNER: LodFilter = LodFilter::Max;
    }
}
