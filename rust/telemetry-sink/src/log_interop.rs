use micromegas_tracing::{
    dispatch::{flush_log_buffer, log_enabled, log_interop},
    error,
    levels::{Level, LevelFilter},
    logs::{LogMetadata, FILTER_LEVEL_UNSET_VALUE},
};
use std::sync::atomic::AtomicU32;

struct LogDispatch;

impl log::Log for LogDispatch {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let level = log_level_to_mm_tracing_level(metadata.level());
        let log_metadata = LogMetadata {
            level,
            level_filter: AtomicU32::new(0),
            fmt_str: "",
            target: "unknown",
            module_path: "unknown",
            file: "unknown",
            line: 0,
        };
        log_enabled(&log_metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        let level = log_level_to_mm_tracing_level(record.level());
        let log_desc = LogMetadata {
            level,
            level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
            fmt_str: record.args().as_str().unwrap_or(""),
            target: record.target(),
            module_path: record.module_path_static().unwrap_or("unknown"),
            file: record.file_static().unwrap_or("unknown"),
            line: record.line().unwrap_or(0),
        };
        log_interop(&log_desc, *record.args());
    }
    fn flush(&self) {
        flush_log_buffer();
    }
}

pub fn install_log_interop(interop_max_level_override: Option<LevelFilter>) {
    /// Installs a `log` crate dispatcher that forwards log records to the Micromegas tracing system.
    ///
    /// This allows applications using the `log` crate to integrate with Micromegas telemetry.
    ///
    /// # Arguments
    ///
    /// * `interop_max_level_override` - An optional `LevelFilter` to override the maximum log level
    ///   for the `log` crate dispatcher. If `None`, the global Micromegas max level is used.
    static LOG_DISPATCHER: LogDispatch = LogDispatch;
    let interop_max_level = mm_tracing_level_filter_to_log_level_filter(
        interop_max_level_override.unwrap_or(micromegas_tracing::levels::max_level()),
    );
    log::set_max_level(interop_max_level);

    if let Err(e) = log::set_logger(&LOG_DISPATCHER) {
        error!("Could not set log crate dispatcher {e:?}");
        log::set_max_level(log::LevelFilter::Off);
    }
}

fn log_level_to_mm_tracing_level(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::Error,
        log::Level::Warn => Level::Warn,
        log::Level::Info => Level::Info,
        log::Level::Debug => Level::Debug,
        log::Level::Trace => Level::Trace,
    }
}

pub(crate) fn mm_tracing_level_filter_to_log_level_filter(level: LevelFilter) -> log::LevelFilter {
    match level {
        LevelFilter::Off => log::LevelFilter::Off,
        LevelFilter::Fatal => log::LevelFilter::Off, //there is no fatal level in the log crate
        LevelFilter::Error => log::LevelFilter::Error,
        LevelFilter::Warn => log::LevelFilter::Warn,
        LevelFilter::Info => log::LevelFilter::Info,
        LevelFilter::Debug => log::LevelFilter::Debug,
        LevelFilter::Trace => log::LevelFilter::Trace,
    }
}
