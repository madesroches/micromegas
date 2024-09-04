use super::{
    log_view::LogViewMaker, metrics_view::MetricsViewMaker, processes_view::ProcessesViewMaker,
    streams_view::StreamsViewMaker, thread_spans_view::ThreadSpansViewMaker, view::View,
};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};

pub trait ViewMaker: Send + Sync {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>>;
}

pub struct ViewFactory {
    view_sets: HashMap<String, Arc<dyn ViewMaker>>,
}

impl ViewFactory {
    pub fn new() -> Self {
        Self {
            view_sets: HashMap::new(),
        }
    }

    pub fn add_view_set(&mut self, view_set_name: String, maker: Arc<dyn ViewMaker>) {
        self.view_sets.insert(view_set_name, maker);
    }

    pub fn make_view(&self, view_set_name: &str, view_instance_id: &str) -> Result<Arc<dyn View>> {
        if let Some(maker) = self.view_sets.get(view_set_name) {
            maker.make_view(view_instance_id)
        } else {
            anyhow::bail!("view set {view_set_name} not found");
        }
    }
}

impl Default for ViewFactory {
    fn default() -> Self {
        let mut factory = Self::new();
        factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
        factory.add_view_set(String::from("measures"), Arc::new(MetricsViewMaker {}));
        factory.add_view_set(
            String::from("thread_spans"),
            Arc::new(ThreadSpansViewMaker {}),
        );
        factory.add_view_set(String::from("processes"), Arc::new(ProcessesViewMaker {}));
        factory.add_view_set(String::from("streams"), Arc::new(StreamsViewMaker {}));
        factory
    }
}
