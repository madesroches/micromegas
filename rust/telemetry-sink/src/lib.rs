//! Telemetry Grpc sink library
//!
//! Provides logging, metrics, memory and performance profiling

// crate-specific lint exceptions:
#![allow(unsafe_code, clippy::missing_errors_doc, clippy::new_without_default)]

use std::any::TypeId;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};

pub mod composite_event_sink;
pub mod http_event_sink;
pub mod local_event_sink;
pub mod log_interop;
pub mod request_decorator;
pub mod stream_block;
pub mod stream_info;
pub mod system_monitor;
pub mod tracing_interop;

use crate::log_interop::install_log_interop;
use crate::request_decorator::RequestDecorator;
use crate::tracing_interop::install_tracing_interop;
use micromegas_tracing::event::BoxedEventSink;
use micromegas_tracing::info;
use micromegas_tracing::{
    event::EventSink,
    guards::{TracingSystemGuard, TracingThreadGuard},
    prelude::*,
};

use composite_event_sink::CompositeSink;
use local_event_sink::LocalEventSink;
use system_monitor::spawn_system_monitor;

pub mod tokio_retry {
    pub use tokio_retry2::*;
}

pub mod reqwest {
    pub use reqwest::*;
}

use crate::http_event_sink::HttpEventSink;

pub struct TelemetryGuardBuilder {
    logs_buffer_size: usize,
    metrics_buffer_size: usize,
    threads_buffer_size: usize,
    target_max_levels: HashMap<String, String>,
    max_queue_size: isize,
    max_level_override: Option<LevelFilter>,
    interop_max_level_override: Option<LevelFilter>,
    install_log_capture: bool,
    install_tracing_capture: bool,
    local_sink_enabled: bool,
    local_sink_max_level: LevelFilter,
    telemetry_sink_url: Option<String>,
    telemetry_sink_max_level: LevelFilter,
    telemetry_metadata_retry: Option<core::iter::Take<tokio_retry::strategy::ExponentialBackoff>>,
    telemetry_make_request_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    extra_sinks: HashMap<TypeId, (LevelFilter, BoxedEventSink)>,
    system_metrics_enabled: bool,
}

impl Default for TelemetryGuardBuilder {
    fn default() -> Self {
        Self {
            logs_buffer_size: 10 * 1024 * 1024,
            metrics_buffer_size: 1024 * 1024,
            threads_buffer_size: 10 * 1024 * 1024,
            local_sink_enabled: true,
            local_sink_max_level: LevelFilter::Info,
            telemetry_sink_url: None,
            telemetry_sink_max_level: LevelFilter::Debug,
            telemetry_metadata_retry: None,
            telemetry_make_request_decorator: Box::new(|| {
                Arc::new(request_decorator::TrivialRequestDecorator {})
            }),
            target_max_levels: HashMap::default(),
            max_queue_size: 16, //todo: change to nb_threads * 2
            max_level_override: None,
            interop_max_level_override: None,
            install_log_capture: false,
            install_tracing_capture: true,
            extra_sinks: HashMap::default(),
            system_metrics_enabled: true,
        }
    }
}

impl TelemetryGuardBuilder {
    // Only one sink per type ?
    #[must_use]
    pub fn add_sink<Sink>(mut self, max_level: LevelFilter, sink: Sink) -> Self
    where
        Sink: EventSink + 'static,
    {
        let type_id = TypeId::of::<Sink>();

        self.extra_sinks
            .entry(type_id)
            .or_insert_with(|| (max_level, Box::new(sink)));

        self
    }

    /// Programmatic override
    #[must_use]
    pub fn with_max_level_override(mut self, level_filter: LevelFilter) -> Self {
        self.max_level_override = Some(level_filter);
        self
    }

    #[must_use]
    pub fn with_local_sink_enabled(mut self, enabled: bool) -> Self {
        self.local_sink_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_interop_max_level_override(mut self, level_filter: LevelFilter) -> Self {
        self.interop_max_level_override = Some(level_filter);
        self
    }

    #[must_use]
    pub fn with_install_log_capture(mut self, enabled: bool) -> Self {
        self.install_log_capture = enabled;
        self
    }

    #[must_use]
    pub fn with_install_tracing_capture(mut self, enabled: bool) -> Self {
        self.install_tracing_capture = enabled;
        self
    }

    #[must_use]
    pub fn with_local_sink_max_level(mut self, level_filter: LevelFilter) -> Self {
        self.local_sink_max_level = level_filter;
        self
    }

    #[must_use]
    pub fn with_ctrlc_handling(self) -> Self {
        ctrlc::set_handler(move || {
            info!("Ctrl+C was hit!");
            micromegas_tracing::guards::shutdown_telemetry();
            std::process::exit(1);
        })
        .expect("Error setting Ctrl+C handler");
        self
    }

    #[must_use]
    pub fn with_telemetry_metadata_retry(
        mut self,
        retry_strategy: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
    ) -> Self {
        self.telemetry_metadata_retry = Some(retry_strategy);
        self
    }

    #[must_use]
    pub fn with_request_decorator(
        mut self,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    ) -> Self {
        self.telemetry_make_request_decorator = make_decorator;
        self
    }

    #[must_use]
    pub fn with_system_metrics_enabled(mut self, enabled: bool) -> Self {
        self.system_metrics_enabled = enabled;
        self
    }

    /// Set the URL of telemetry sink.
    ///
    /// If not explicitly set, the URL will be read from the `MICROMEGAS_TELEMETRY_URL` environment
    /// variable.
    #[must_use]
    pub fn with_telemetry_sink_url(mut self, url: String) -> Self {
        self.telemetry_sink_url = Some(url);
        self
    }

    pub fn build(self) -> anyhow::Result<TelemetryGuard> {
        let target_max_level: Vec<_> = self
            .target_max_levels
            .into_iter()
            .filter(|(key, _val)| key != "MAX_LEVEL")
            .map(|(key, val)| {
                (
                    key,
                    LevelFilter::from_str(val.as_str()).unwrap_or(LevelFilter::Off),
                )
            })
            .collect();

        let guard = {
            lazy_static::lazy_static! {
                static ref GLOBAL_WEAK_GUARD: Mutex<Weak<TracingSystemGuard>> = Mutex::new(Weak::new());
            }
            let mut weak_guard = GLOBAL_WEAK_GUARD.lock().unwrap();
            let weak = &mut *weak_guard;

            if let Some(arc) = weak.upgrade() {
                arc
            } else {
                let mut sinks: Vec<(LevelFilter, BoxedEventSink)> = vec![];
                let telemetry_sink_url = self
                    .telemetry_sink_url
                    .or_else(|| std::env::var("MICROMEGAS_TELEMETRY_URL").ok());

                if let Some(url) = telemetry_sink_url {
                    let retry_strategy = self.telemetry_metadata_retry.unwrap_or_else(|| {
                        tokio_retry::strategy::ExponentialBackoff::from_millis(10).take(3)
                    });
                    sinks.push((
                        self.telemetry_sink_max_level,
                        Box::new(HttpEventSink::new(
                            &url,
                            self.max_queue_size,
                            retry_strategy,
                            self.telemetry_make_request_decorator,
                        )),
                    ));
                }
                if self.local_sink_enabled {
                    sinks.push((self.local_sink_max_level, Box::new(LocalEventSink::new())));
                }
                let mut extra_sinks = self.extra_sinks.into_values().collect();
                sinks.append(&mut extra_sinks);

                let sink: BoxedEventSink = Box::new(CompositeSink::new(
                    sinks,
                    target_max_level,
                    self.max_level_override,
                ));

                // the composite sink inits micromegas_tracing::levels::set_max_level, which install_log_interop needs
                if self.install_log_capture {
                    install_log_interop(self.interop_max_level_override);
                }
                if self.install_tracing_capture {
                    install_tracing_interop(self.interop_max_level_override);
                }

                let arc = Arc::<TracingSystemGuard>::new(TracingSystemGuard::new(
                    self.logs_buffer_size,
                    self.metrics_buffer_size,
                    self.threads_buffer_size,
                    sink.into(),
                )?);

                if self.system_metrics_enabled {
                    spawn_system_monitor();
                }

                *weak = Arc::<TracingSystemGuard>::downgrade(&arc);
                arc
            }
        };
        // order here is important
        Ok(TelemetryGuard {
            _guard: guard,
            _thread_guard: TracingThreadGuard::new(),
        })
    }
}

pub struct TelemetryGuard {
    // note we rely here on the drop order being the same as the declaration order
    _thread_guard: TracingThreadGuard,
    _guard: Arc<TracingSystemGuard>,
}

impl TelemetryGuard {
    pub fn new() -> anyhow::Result<Self> {
        TelemetryGuardBuilder::default().build()
    }
}
