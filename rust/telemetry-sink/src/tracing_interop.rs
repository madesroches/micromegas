use micromegas_tracing::{
    dispatch::{
        has_thread_stream, log_interop, log_interop_dyn_tagged, on_begin_async_named_scope_dyn,
        on_end_async_named_scope_tagged,
    },
    levels::{LevelFilter, Verbosity},
    logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata},
    property_set::{DynProperty, PropertySet},
    spans::SpanLocation,
};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock, atomic::AtomicU32};
use tracing_subscriber::{layer::Context, prelude::*, registry::LookupSpan};

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

/// Decides what is stored for one span field value: `None` drops the field, `Some` keeps the
/// returned value.
///
/// Arguments are the span's callsite metadata, the field name and the value as text (the `Debug`
/// form for non-string values).
pub type SpanFieldHook = Box<
    dyn for<'v> Fn(&'static tracing::Metadata<'static>, &str, &'v str) -> Option<Cow<'v, str>>
        + Send
        + Sync,
>;

type SpanFieldHookRef<'a> = &'a (
        dyn for<'v> Fn(&'static tracing::Metadata<'static>, &str, &'v str) -> Option<Cow<'v, str>>
            + Send
            + Sync
    );

/// A tracing layer compatible with `tracing_subscriber`.
///
/// Setting-up this layer still requires the proper initialization of a `TelemetryGuard`.
pub struct TracingCaptureLayer {
    pub max_level: LevelFilter,
    /// Maps each `tracing` span to an async span, with its fields as end-event properties.
    /// Only recorded when CPU tracing is enabled and on threads that have a Micromegas thread
    /// stream (e.g. runtime threads or a thread holding a `TracingThreadGuard`); spans on other
    /// threads are not recorded.
    pub capture_spans: bool,
    /// Called for each span field value before it is stored.
    pub span_field_hook: Option<SpanFieldHook>,
}

/// Per-span state kept in the registry between `on_new_span` and `on_close`.
struct CapturedSpan {
    span_id: u64,
    parent_span_id: u64,
    depth: u32,
    location: &'static SpanLocation,
    fields: Vec<(&'static str, String)>,
}

/// Records span fields into owned storage, because they must outlive the callback that saw them.
/// A `None` value means the hook dropped the field, which a later record must be able to express.
struct SpanFieldVisitor<'a> {
    metadata: &'static tracing::Metadata<'static>,
    hook: Option<SpanFieldHookRef<'a>>,
    fields: &'a mut Vec<(&'static str, Option<String>)>,
}

fn upsert<V>(fields: &mut Vec<(&'static str, V)>, name: &'static str, value: V) {
    match fields.iter_mut().find(|(n, _)| *n == name) {
        Some(entry) => entry.1 = value,
        None => fields.push((name, value)),
    }
}

/// Whether the hook returned its input untouched, so that it need not be copied again. The data
/// pointer is compared as well as the length, because a same-length static replacement is not
/// the input; an owned return never shares the input's address.
fn is_whole_input(returned: &str, input: &str) -> bool {
    std::ptr::eq(returned.as_ptr(), input.as_ptr()) && returned.len() == input.len()
}

impl Visit for SpanFieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name().starts_with("log.") {
            return;
        }
        let stored = match self.hook {
            None => Some(value.to_owned()),
            Some(hook) => hook(self.metadata, field.name(), value).map(Cow::into_owned),
        };
        upsert(self.fields, field.name(), stored);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name().starts_with("log.") {
            return;
        }
        let formatted = format!("{value:?}");
        let stored = match self.hook {
            None => Some(formatted),
            Some(hook) => match hook(self.metadata, field.name(), &formatted) {
                None => None,
                Some(returned) if is_whole_input(&returned, &formatted) => {
                    drop(returned);
                    Some(formatted)
                }
                Some(returned) => Some(returned.into_owned()),
            },
        };
        upsert(self.fields, field.name(), stored);
    }
}

/// Span locations are keyed by address downstream, so each callsite needs one that never moves.
fn span_location(metadata: &'static tracing::Metadata<'static>) -> &'static SpanLocation {
    static LOCATIONS: OnceLock<Mutex<HashMap<usize, &'static SpanLocation>>> = OnceLock::new();
    let mut locations = LOCATIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Leaked once per callsite, so the total is bounded by the program text.
    locations
        .entry(std::ptr::from_ref(metadata) as usize)
        .or_insert_with(|| {
            Box::leak(Box::new(SpanLocation {
                lod: tracing_level_to_verbosity(metadata.level()),
                target: metadata.target(),
                module_path: metadata.module_path().unwrap_or_default(),
                file: metadata.file().unwrap_or_default(),
                line: metadata.line().unwrap_or(0),
            }))
        })
}

impl<S> tracing_subscriber::Layer<S> for TracingCaptureLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        // A combined layer stack still creates spans that this layer's `enabled` rejected.
        // The layer never creates a thread stream: one registered by a thread that then exits
        // would be left dangling in the dispatch, so only threads that already own one record.
        if !self.capture_spans
            || !has_thread_stream()
            || tracing_level_to_mm_level(attrs.metadata().level()) > self.max_level
        {
            return;
        }
        let Some(span) = ctx.span(id) else {
            return;
        };
        let (parent_span_id, depth) = span
            .scope()
            .skip(1)
            .find_map(|ancestor| {
                ancestor
                    .extensions()
                    .get::<CapturedSpan>()
                    .map(|parent| (parent.span_id, parent.depth + 1))
            })
            .unwrap_or((0, 0));
        let location = span_location(span.metadata());
        let name = span.name();
        let mut recorded = vec![];
        attrs.record(&mut SpanFieldVisitor {
            metadata: attrs.metadata(),
            hook: self.span_field_hook.as_deref(),
            fields: &mut recorded,
        });
        let fields = recorded
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect();
        // The registry borrow is released before dispatching, since a sink may emit spans too.
        drop(span);

        let span_id = on_begin_async_named_scope_dyn(location, name, parent_span_id, depth);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(CapturedSpan {
                span_id,
                parent_span_id,
                depth,
                location,
                fields,
            });
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if span.extensions().get::<CapturedSpan>().is_none() {
            return;
        }
        // Recorded without holding the extensions lock, because a `Debug` impl may create a
        // span, whose `on_new_span` reads the extensions of its ancestors.
        let mut recorded = vec![];
        values.record(&mut SpanFieldVisitor {
            metadata: span.metadata(),
            hook: self.span_field_hook.as_deref(),
            fields: &mut recorded,
        });
        let mut extensions = span.extensions_mut();
        if let Some(captured) = extensions.get_mut::<CapturedSpan>() {
            for (name, value) in recorded {
                match value {
                    Some(value) => upsert(&mut captured.fields, name, value),
                    None => captured.fields.retain(|(n, _)| *n != name),
                }
            }
        }
    }

    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let name = span.name();
        let captured = span.extensions_mut().remove::<CapturedSpan>();
        drop(span);
        let Some(captured) = captured else {
            return;
        };
        // A span dropped during thread-local teardown would panic (and abort) when the end
        // event reaches the already-destroyed thread stream.
        if !has_thread_stream() {
            return;
        }

        let dyn_properties: SmallVec<[DynProperty<'_>; INLINE_FIELDS]> = captured
            .fields
            .iter()
            .map(|(name, value)| DynProperty::new(name, value))
            .collect();
        on_end_async_named_scope_tagged(
            captured.span_id,
            captured.parent_span_id,
            captured.location,
            name,
            captured.depth,
            PropertySet::empty(),
            &dyn_properties,
        );
    }

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
///
/// `tracing` spans are recorded as async spans, but only when CPU tracing is enabled
/// (`MICROMEGAS_ENABLE_CPU_TRACING`) and only on threads that have a Micromegas thread stream
/// (e.g. runtime threads or a thread holding a `TracingThreadGuard`); spans on other threads are
/// not recorded. Build a `TracingCaptureLayer` with `capture_spans: false` to forward events only.
pub fn install_tracing_interop(interop_max_level_override: Option<LevelFilter>) {
    let max_level = interop_max_level_override.unwrap_or(micromegas_tracing::levels::max_level());

    tracing_subscriber::registry()
        .with(TracingCaptureLayer {
            max_level,
            capture_spans: true,
            span_field_hook: None,
        })
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

fn tracing_level_to_verbosity(level: &tracing_core::Level) -> Verbosity {
    match *level {
        tracing_core::Level::ERROR | tracing_core::Level::WARN => Verbosity::Min,
        tracing_core::Level::INFO => Verbosity::Med,
        tracing_core::Level::DEBUG | tracing_core::Level::TRACE => Verbosity::Max,
    }
}
