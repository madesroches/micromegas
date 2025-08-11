use super::{EventSink, StreamDesc};
use crate::{
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::Property,
    spans::{ThreadBlock, ThreadStream},
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

pub struct MemSinkState {
    pub process_info: Option<Arc<ProcessInfo>>,
    pub log_stream_desc: Option<Arc<StreamDesc>>,
    pub metrics_stream_desc: Option<Arc<StreamDesc>>,
    pub thread_stream_descs: Vec<Arc<StreamDesc>>,
    pub thread_blocks: Vec<Arc<ThreadBlock>>,
}

/// for tests where we want to inspect the collected data
pub struct InMemorySink {
    pub state: Mutex<MemSinkState>,
}

impl InMemorySink {
    pub fn new() -> Self {
        let state = MemSinkState {
            process_info: None,
            log_stream_desc: None,
            metrics_stream_desc: None,
            thread_stream_descs: vec![],
            thread_blocks: vec![],
        };
        Self {
            state: Mutex::new(state),
        }
    }
}

impl Default for InMemorySink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for InMemorySink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        self.state.lock().unwrap().process_info = Some(process_info);
    }

    fn on_shutdown(&self) {}

    fn on_log_enabled(&self, _metadata: &LogMetadata) -> bool {
        todo!()
    }

    fn on_log(
        &self,
        _desc: &LogMetadata,
        _properties: &[Property],
        _time: i64,
        _args: fmt::Arguments<'_>,
    ) {
        todo!()
    }

    fn on_init_log_stream(&self, log_stream: &LogStream) {
        self.state.lock().unwrap().log_stream_desc = Some(log_stream.desc());
    }

    fn on_process_log_block(&self, _log_block: Arc<LogBlock>) {
        todo!()
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.state.lock().unwrap().metrics_stream_desc = Some(metrics_stream.desc());
    }

    fn on_process_metrics_block(&self, _metrics_block: Arc<MetricsBlock>) {
        todo!()
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.state
            .lock()
            .unwrap()
            .thread_stream_descs
            .push(thread_stream.desc());
    }

    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>) {
        self.state.lock().unwrap().thread_blocks.push(thread_block);
    }

    fn is_busy(&self) -> bool {
        todo!()
    }
}
