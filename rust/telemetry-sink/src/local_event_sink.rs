use std::{fmt, sync::Arc};

use tracing::{
    event::EventSink,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    spans::{ThreadBlock, ThreadStream},
    Level, ProcessInfo,
};

// Based on simple logger
#[cfg(feature = "colored")]
use colored::Colorize;

#[cfg(feature = "timestamps")]
use time::{format_description::FormatItem, OffsetDateTime};

#[cfg(feature = "timestamps")]
const TIMESTAMP_FORMAT_UTC: &[FormatItem<'_>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
);

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
    pub fn new() -> Self {
        #[cfg(all(windows, feature = "colored"))]
        set_up_color_terminal();

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

    fn on_log(&self, metadata: &LogMetadata, _time: i64, args: fmt::Arguments<'_>) {
        let level_string = {
            #[cfg(feature = "colored")]
            {
                if self.colors {
                    match metadata.level {
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

        let target = if !metadata.target.is_empty() {
            metadata.target
        } else {
            metadata.module_path
        };

        let timestamp = {
            #[cfg(feature = "timestamps")]
            if self.timestamps {
                format!(
                    "{} ",
                    OffsetDateTime::now_utc()
                        .format(&TIMESTAMP_FORMAT_UTC)
                        .unwrap()
                )
            } else {
                "".to_string()
            }

            #[cfg(not(feature = "timestamps"))]
            ""
        };

        let message = format!("{}{:<5} [{}] {}", timestamp, level_string, target, args);

        #[cfg(not(feature = "stderr"))]
        println!("{}", message);

        #[cfg(feature = "stderr")]
        eprintln!("{}", message);
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

#[cfg(all(windows, feature = "colored"))]
fn set_up_color_terminal() {
    use atty::Stream;

    if atty::is(Stream::Stdout) {
        unsafe {
            let stdout =
                winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE);

            if stdout == winapi::um::handleapi::INVALID_HANDLE_VALUE {
                return;
            }

            let mut mode: winapi::shared::minwindef::DWORD = 0;

            if winapi::um::consoleapi::GetConsoleMode(stdout, &mut mode) == 0 {
                return;
            }

            winapi::um::consoleapi::SetConsoleMode(
                stdout,
                mode | winapi::um::wincon::ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            );
        }
    }
}
