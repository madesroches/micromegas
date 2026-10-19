use micromegas_tracing::{
    dispatch::{log_interop, log_interop_dyn_tagged},
    levels::LevelFilter,
    logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata},
    property_set::DynProperty,
};
use smallvec::SmallVec;
use std::cell::RefCell;
use std::ops::Range;
use std::sync::atomic::AtomicU32;
use tracing_subscriber::{layer::Context, prelude::*};

use std::fmt::{self, Write};
use tracing::field::{Field, Visit};

const INLINE_FIELDS: usize = 4;

/// Larger scratch buffers are dropped after the event instead of being kept for the thread's
/// lifetime.
const MAX_RETAINED_SCRATCH: usize = 4096;

pub struct FieldFormatVisitor<'a> {
    pub buffer: &'a mut String,
    pub target: Option<String>,
    /// Values of the event fields that become dynamic properties, concatenated.
    pub values: &'a mut String,
    /// Event fields that become dynamic properties, in addition to their message text, as
    /// ranges into `values`.
    pub properties: SmallVec<[(&'static str, Range<usize>); INLINE_FIELDS]>,
}

impl FieldFormatVisitor<'_> {
    fn write_message_part(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            write!(self.buffer, "{value:?} ").expect("formatting into a String");
        } else {
            write!(self.buffer, "{}={:?} ", field.name(), value).expect("formatting into a String");
        }
    }

    // `log.*` fields only carry the origin of a bridged `log` record.
    fn is_property(field: &Field) -> bool {
        field.name() != "message" && !field.name().starts_with("log.")
    }
}

impl Visit for FieldFormatVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "log.target" {
            self.target = Some(value.to_owned());
            return;
        }
        if Self::is_property(field) {
            let start = self.values.len();
            self.values.push_str(value);
            self.properties
                .push((field.name(), start..self.values.len()));
        }
        self.write_message_part(field, &value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !Self::is_property(field) {
            self.write_message_part(field, value);
            return;
        }
        // Formatted once into `values`; the message reuses that text.
        let start = self.values.len();
        write!(self.values, "{value:?}").expect("formatting into a String");
        let range = start..self.values.len();
        self.buffer.push_str(field.name());
        self.buffer.push('=');
        self.buffer.push_str(&self.values[range.clone()]);
        self.buffer.push(' ');
        self.properties.push((field.name(), range));
    }
}

#[derive(Default)]
struct Scratch {
    buffer: String,
    values: String,
}

thread_local! {
    // Reused across events so that capturing one does not allocate its working buffers.
    static SCRATCH: RefCell<Scratch> = RefCell::default();
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
        let capture = |scratch: &mut Scratch| capture_event(event, level, scratch);
        // A sink that emits `tracing` events re-enters here while the scratch is borrowed, and
        // thread-local destruction makes it unavailable; both fall back to fresh buffers.
        let reused = SCRATCH
            .try_with(|cell| match cell.try_borrow_mut() {
                Ok(mut scratch) => {
                    // The buffers keep the previous event's contents, which also covers a capture that panicked.
                    scratch.buffer.clear();
                    scratch.values.clear();
                    capture(&mut scratch);
                    if scratch.buffer.capacity() > MAX_RETAINED_SCRATCH
                        || scratch.values.capacity() > MAX_RETAINED_SCRATCH
                    {
                        *scratch = Scratch::default();
                    }
                    true
                }
                Err(_) => false,
            })
            .unwrap_or(false);
        if !reused {
            capture(&mut Scratch::default());
        }
    }
}

fn capture_event(
    event: &tracing::Event<'_>,
    level: micromegas_tracing::levels::Level,
    scratch: &mut Scratch,
) {
    let mut formatter = FieldFormatVisitor {
        buffer: &mut scratch.buffer,
        target: None,
        values: &mut scratch.values,
        properties: SmallVec::new(),
    };
    event.record(&mut formatter);
    let FieldFormatVisitor {
        buffer,
        target,
        values,
        properties,
    } = formatter;
    let log_desc = LogMetadata {
        level,
        level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "{}",
        target: target.as_deref().unwrap_or(event.metadata().target()),
        module_path: event.metadata().module_path().unwrap_or_default(),
        file: "",
        line: 0,
    };

    if properties.is_empty() {
        log_interop(&log_desc, format_args!("{buffer}"));
    } else {
        let dyn_properties: SmallVec<[DynProperty<'_>; INLINE_FIELDS]> = properties
            .iter()
            .map(|(name, range)| DynProperty::new(name, &values[range.clone()]))
            .collect();
        log_interop_dyn_tagged(&log_desc, &dyn_properties, format_args!("{buffer}"));
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
