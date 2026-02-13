use std::{fmt, sync::Arc};

use micromegas_tracing::event::EventSink;
use micromegas_tracing::logs::{LogBlock, LogMetadata, LogStream};
use micromegas_tracing::metrics::{MetricsBlock, MetricsStream};
use micromegas_tracing::process_info::ProcessInfo;
use micromegas_tracing::property_set::Property;
use micromegas_tracing::spans::{ThreadBlock, ThreadStream};

pub struct ConsoleEventSink;

impl EventSink for ConsoleEventSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        true
    }

    fn on_log(
        &self,
        desc: &LogMetadata,
        _properties: &[Property],
        _time: i64,
        args: fmt::Arguments<'_>,
    ) {
        let msg = format!("[{}] {args}", desc.level);
        web_sys::console::log_1(&msg.into());
    }

    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, _: Arc<LogBlock>) {}
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, _: Arc<MetricsBlock>) {}
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool {
        false
    }
}
