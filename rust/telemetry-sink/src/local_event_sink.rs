use micromegas_tracing::{
    event::EventSink,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::{Property, property_get},
    spans::{ThreadBlock, ThreadStream},
};
use std::{fmt, sync::Arc};

use colored::Colorize;

/// Prints log entries to the console
pub struct LocalEventSink {}

impl LocalEventSink {
    pub fn new() -> Self {
        Self {}
    }
}

impl EventSink for LocalEventSink {
    fn on_startup(&self, _proc_info: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}

    fn on_log_enabled(&self, _metadata: &LogMetadata) -> bool {
        // reaching here we accept everything
        true
    }

    fn on_log(
        &self,
        metadata: &LogMetadata,
        properties: &[Property],
        _time: i64,
        args: fmt::Arguments<'_>,
    ) {
        let level_string = match metadata.level {
            Level::Fatal => metadata.level.to_string().red().to_string(),
            Level::Error => metadata.level.to_string().red().to_string(),
            Level::Warn => metadata.level.to_string().yellow().to_string(),
            Level::Info => metadata.level.to_string().cyan().to_string(),
            Level::Debug => metadata.level.to_string().purple().to_string(),
            Level::Trace => metadata.level.to_string().normal().to_string(),
        };

        let mut target = if !metadata.target.is_empty() {
            metadata.target
        } else {
            metadata.module_path
        };

        if let Some(t) = property_get(properties, "target") {
            target = t;
        }

        let timestamp = format!("{} ", chrono::Utc::now().to_rfc3339());

        let message = format!("{timestamp}{level_string:<5} [{target}] {args}");

        println!("{message}");
    }

    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, _: Arc<LogBlock>) {}

    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, _: Arc<MetricsBlock>) {}

    fn on_init_thread_stream(&self, _thread_stream: &ThreadStream) {}

    #[allow(clippy::cast_precision_loss)]
    fn on_process_thread_block(&self, _block: Arc<ThreadBlock>) {}

    fn is_busy(&self) -> bool {
        false
    }
}
