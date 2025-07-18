use micromegas_tracing::{
    dispatch::log_interop,
    levels::LevelFilter,
    logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata},
};
use std::sync::atomic::AtomicU32;
use tracing_subscriber::{layer::Context, prelude::*};

use std::fmt::{self, Write};
use tracing::field::{Field, Visit};
pub struct FieldFormatVisitor<'a> {
    pub buffer: &'a mut String,
    pub target: Option<String>,
}

impl Visit for FieldFormatVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "log.target" {
            self.target = Some(value.to_owned());
        } else {
            self.record_debug(field, &value)
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            write!(self.buffer, "{value:?} ").unwrap();
        } else {
            write!(self.buffer, "{}={:?} ", field.name(), value).unwrap();
        }
    }
}

/// A tracing layer compatible with `tracing_subscriber`.
///
/// Setting-up this layer still requires the proper initialization of a `TelemetryGuard`.
pub struct TracingCaptureLayer {
    pub max_level: LevelFilter,
}

impl<S> tracing_subscriber::Layer<S> for TracingCaptureLayer
where
    S: tracing::Subscriber,
{
    fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        let level = tracing_level_to_mm_level(metadata.level());
        level <= self.max_level
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = tracing_level_to_mm_level(event.metadata().level());
        if level > self.max_level {
            return;
        }
        let mut buffer = String::new();
        let mut formatter = FieldFormatVisitor {
            buffer: &mut buffer,
            target: None,
        };
        event.record(&mut formatter);
        let log_desc = LogMetadata {
            level,
            level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
            fmt_str: "{}",
            target: formatter
                .target
                .as_deref()
                .unwrap_or(event.metadata().target()),
            module_path: event.metadata().module_path().unwrap_or_default(),
            file: "",
            line: 0,
        };

        log_interop(&log_desc, format_args!("{}", &buffer));
    }
}

/// Installs a `tracing` layer that forwards events to the Micromegas tracing system.
///
/// This allows applications using the `tracing` crate to integrate with Micromegas telemetry.
///
/// # Arguments
///
/// * `interop_max_level_override` - An optional `LevelFilter` to override the maximum level
///   for the `tracing` layer. If `None`, the global Micromegas max level is used.
pub fn install_tracing_interop(interop_max_level_override: Option<LevelFilter>) {
    let max_level = interop_max_level_override.unwrap_or(micromegas_tracing::levels::max_level());

    tracing_subscriber::registry()
        .with(TracingCaptureLayer { max_level })
        .init();
    tracing::debug!("installed tracing interop");
}

fn tracing_level_to_mm_level(level: &tracing_core::Level) -> micromegas_tracing::levels::Level {
    match *level {
        tracing_core::Level::ERROR => micromegas_tracing::levels::Level::Error,
        tracing_core::Level::WARN => micromegas_tracing::levels::Level::Warn,
        tracing_core::Level::INFO => micromegas_tracing::levels::Level::Info,
        tracing_core::Level::DEBUG => micromegas_tracing::levels::Level::Debug,
        tracing_core::Level::TRACE => micromegas_tracing::levels::Level::Trace,
    }
}
