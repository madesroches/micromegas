use micromegas_tracing::{
    event::EventSink,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::{Property, property_get},
    spans::{ThreadBlock, ThreadStream},
};
use std::{fmt, sync::Arc};

// Based on simple logger
#[cfg(feature = "colored")]
use colored::Colorize;

pub struct LocalEventSink {
    /// Control how timestamps are displayed.
    ///
    /// This field is only available if the `timestamps` feature is enabled.
    #[cfg(feature = "timestamps")]
    timestamps: bool,

    /// Whether to use color output or not.
    ///
    /// This field is only available if the `color` feature is enabled.
    #[cfg(feature = "colored")]
    colors: bool,
}

impl LocalEventSink {
    /// Creates a new `LocalEventSink`.
    ///
    /// Initializes the sink with default settings for timestamps and colors.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "timestamps")]
            timestamps: true,
            #[cfg(feature = "colored")]
            colors: true,
        }
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
        let level_string = {
            #[cfg(feature = "colored")]
            {
                if self.colors {
                    match metadata.level {
                        Level::Fatal => metadata.level.to_string().red().to_string(),
                        Level::Error => metadata.level.to_string().red().to_string(),
                        Level::Warn => metadata.level.to_string().yellow().to_string(),
                        Level::Info => metadata.level.to_string().cyan().to_string(),
                        Level::Debug => metadata.level.to_string().purple().to_string(),
                        Level::Trace => metadata.level.to_string().normal().to_string(),
                    }
                } else {
                    metadata.level.to_string()
                }
            }
            #[cfg(not(feature = "colored"))]
            {
                record.level().to_string()
            }
        };

        let mut target = if !metadata.target.is_empty() {
            metadata.target
        } else {
            metadata.module_path
        };

        if let Some(t) = property_get(properties, "target") {
            target = t;
        }

        let timestamp = {
            #[cfg(feature = "timestamps")]
            if self.timestamps {
                format!("{} ", chrono::Utc::now().to_rfc3339())
            } else {
                "".to_string()
            }

            #[cfg(not(feature = "timestamps"))]
            ""
        };

        let message = format!("{timestamp}{level_string:<5} [{target}] {args}");

        #[cfg(not(feature = "stderr"))]
        println!("{message}");

        #[cfg(feature = "stderr")]
        eprintln!("{message}");
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
