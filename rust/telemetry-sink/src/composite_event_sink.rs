use micromegas_tracing::{
    dispatch::{flush_log_buffer, log_enabled, log_interop},
    event::{BoxedEventSink, EventSink},
    logs::{LogBlock, LogMetadata, LogStream, FILTER_LEVEL_UNSET_VALUE},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadStream},
};
use std::{
    fmt,
    sync::{atomic::AtomicU32, Arc},
};

pub struct CompositeSink {
    sinks: Vec<(LevelFilter, BoxedEventSink)>,
    target_level_filters: Vec<(String, LevelFilter)>,
}

impl CompositeSink {
    pub fn new(
        sinks: Vec<(LevelFilter, BoxedEventSink)>,
        target_max_level: Vec<(String, LevelFilter)>,
        max_level_override: Option<LevelFilter>,
        interop_max_level_override: Option<LevelFilter>,
    ) -> Self {
        if let Some(max_level) = max_level_override {
            micromegas_tracing::levels::set_max_level(max_level);
        } else {
            let mut max_level = LevelFilter::Off;
            for (_, level_filter) in &target_max_level {
                max_level = max_level.max(*level_filter);
            }
            for (level_filter, _) in &sinks {
                max_level = max_level.max(*level_filter);
            }
            micromegas_tracing::levels::set_max_level(max_level);
        }
        let interop_max_level = if let Some(max_level) = interop_max_level_override {
            tracing_level_filter_to_log_level_filter(max_level)
        } else {
            tracing_level_filter_to_log_level_filter(micromegas_tracing::levels::max_level())
        };
        log::set_max_level(interop_max_level);

        static LOG_DISPATCHER: LogDispatch = LogDispatch;
        if log::set_logger(&LOG_DISPATCHER).is_err() {
            println!("Could not set log crate dispatcher");
            log::set_max_level(log::LevelFilter::Off);
        }

        let mut target_max_level = target_max_level;
        target_max_level.sort_by_key(|(name, _)| name.len().wrapping_neg());

        Self {
            sinks,
            target_level_filters: target_max_level,
        }
    }

    fn target_max_level(&self, metadata: &LogMetadata) -> Option<LevelFilter> {
        const GENERATION: u16 = 1;
        // At this point we would have already tested the max level on the macro
        match metadata.level_filter(GENERATION) {
            micromegas_tracing::logs::FilterState::Outdated => {
                let level_filter =
                    Self::find_max_match(metadata.target, &self.target_level_filters);
                metadata.set_level_filter(GENERATION, level_filter);
                level_filter
            }
            micromegas_tracing::logs::FilterState::NotSet => None,
            micromegas_tracing::logs::FilterState::Set(level_filter) => Some(level_filter),
        }
    }

    /// This needs to be optimized
    fn find_max_match(
        target: &str,
        level_filters: &[(String, LevelFilter)],
    ) -> Option<LevelFilter> {
        for (t, l) in level_filters.iter() {
            if target.starts_with(t) {
                return Some(*l);
            }
        }
        None
    }
}

impl EventSink for CompositeSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        if self.sinks.len() == 1 {
            self.sinks[0].1.on_startup(process_info);
        } else {
            self.sinks
                .iter()
                .for_each(|(_, sink)| sink.on_startup(process_info.clone()));
        }
    }

    fn on_shutdown(&self) {
        self.sinks.iter().for_each(|(_, sink)| sink.on_shutdown());
    }

    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool {
        // The log is enabled if any of the sinks are enabled
        // If the sinks vec is empty `false` will be returned
        let target_max_level = self.target_max_level(metadata);
        self.sinks.iter().any(|(max_level, sink)| {
            metadata.level <= target_max_level.unwrap_or(*max_level)
                && sink.on_log_enabled(metadata)
        })
    }

    fn on_log(&self, metadata: &LogMetadata, time: i64, args: fmt::Arguments<'_>) {
        let target_max_level = self.target_max_level(metadata);
        self.sinks.iter().for_each(|(max_level, sink)| {
            if metadata.level <= target_max_level.unwrap_or(*max_level)
                && sink.on_log_enabled(metadata)
            {
                sink.on_log(metadata, time, args);
            }
        });
    }

    fn on_init_log_stream(&self, log_stream: &LogStream) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_init_log_stream(log_stream));
    }

    fn on_process_log_block(&self, old_event_block: Arc<LogBlock>) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_process_log_block(old_event_block.clone()));
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_init_metrics_stream(metrics_stream));
    }

    fn on_process_metrics_block(&self, old_event_block: Arc<MetricsBlock>) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_process_metrics_block(old_event_block.clone()));
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_init_thread_stream(thread_stream));
    }

    fn on_process_thread_block(&self, old_event_block: Arc<ThreadBlock>) {
        self.sinks
            .iter()
            .for_each(|(_, sink)| sink.on_process_thread_block(old_event_block.clone()));
    }

    fn is_busy(&self) -> bool {
        for (_, sink) in &self.sinks {
            if sink.is_busy() {
                return true;
            }
        }
        false
    }
}

struct LogDispatch;

impl log::Log for LogDispatch {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let level = log_level_to_tracing_level(metadata.level());
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
        let level = log_level_to_tracing_level(record.level());
        let log_desc = LogMetadata {
            level,
            level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
            fmt_str: record.args().as_str().unwrap_or(""),
            target: record.module_path_static().unwrap_or("unknown"),
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

fn log_level_to_tracing_level(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::Error,
        log::Level::Warn => Level::Warn,
        log::Level::Info => Level::Info,
        log::Level::Debug => Level::Debug,
        log::Level::Trace => Level::Trace,
    }
}

pub(crate) fn tracing_level_filter_to_log_level_filter(level: LevelFilter) -> log::LevelFilter {
    match level {
        LevelFilter::Off => log::LevelFilter::Off,
        LevelFilter::Error => log::LevelFilter::Error,
        LevelFilter::Warn => log::LevelFilter::Warn,
        LevelFilter::Info => log::LevelFilter::Info,
        LevelFilter::Debug => log::LevelFilter::Debug,
        LevelFilter::Trace => log::LevelFilter::Trace,
    }
}
