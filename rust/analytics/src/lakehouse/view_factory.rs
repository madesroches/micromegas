use super::{
    blocks_view::BlocksViewMaker, log_view::LogViewMaker, metrics_view::MetricsViewMaker,
    processes_view::ProcessesViewMaker, streams_view::StreamsViewMaker,
    thread_spans_view::ThreadSpansViewMaker, view::View,
};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};

pub trait ViewMaker: Send + Sync {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>>;
}

pub struct ViewFactory {
    view_sets: HashMap<String, Arc<dyn ViewMaker>>,
    global_views: Vec<Arc<dyn View>>,
}

impl ViewFactory {
    pub fn new(global_views: Vec<Arc<dyn View>>) -> Self {
        Self {
            view_sets: HashMap::new(),
            global_views,
        }
    }

    pub fn get_global_views(&self) -> &[Arc<dyn View>] {
        &self.global_views
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

pub fn default_view_factory() -> Result<ViewFactory> {
    let log_view_maker = Arc::new(LogViewMaker {});
    let metrics_view_maker = Arc::new(MetricsViewMaker {});
    let global_views = vec![
        log_view_maker.make_view("global")?,
        metrics_view_maker.make_view("global")?,
    ];
    let mut factory = ViewFactory::new(global_views);
    factory.add_view_set(String::from("log_entries"), log_view_maker);
    factory.add_view_set(String::from("measures"), metrics_view_maker);
    factory.add_view_set(
        String::from("thread_spans"),
        Arc::new(ThreadSpansViewMaker {}),
    );
    factory.add_view_set(String::from("processes"), Arc::new(ProcessesViewMaker {}));
    factory.add_view_set(String::from("streams"), Arc::new(StreamsViewMaker {}));
    factory.add_view_set(String::from("blocks"), Arc::new(BlocksViewMaker {}));
    Ok(factory)
}
