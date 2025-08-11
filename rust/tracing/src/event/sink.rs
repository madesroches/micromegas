use std::{fmt, sync::Arc};

use crate::{
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::Property,
    spans::{ThreadBlock, ThreadStream},
};

pub type BoxedEventSink = Box<dyn EventSink>;

/// interface needed by the dispatch module to send out telemetry
pub trait EventSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>);
    fn on_shutdown(&self);

    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool;
    fn on_log(
        &self,
        desc: &LogMetadata,
        properties: &[Property],
        time: i64,
        args: fmt::Arguments<'_>,
    );
    fn on_init_log_stream(&self, log_stream: &LogStream);
    fn on_process_log_block(&self, log_block: Arc<LogBlock>);

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream);
    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>);

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream);
    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>);

    fn is_busy(&self) -> bool; // sink is busy writing to disk or network, avoid extra flushing
}

/// for tests where the data can be dropped
pub struct NullEventSink {}

impl EventSink for NullEventSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}

    fn on_log_enabled(&self, _: &LogMetadata) -> bool {
        false
    }
    fn on_log(
        &self,
        _desc: &LogMetadata,
        _properties: &[Property],
        _time: i64,
        _args: fmt::Arguments<'_>,
    ) {
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
