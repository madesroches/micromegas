//! [`default_view_factory`] makes the default [`ViewFactory`], giving users access to view instances, grouped in sets.
//!
//! # View sets
//!
//! A ViewFactory defines the available view sets and can instanciate view instances.
//! All view instances in a set have the same schema.
//! Some view instances are global (their view_instance_id is 'global').
//! Global view instances are implicitly accessible to SQL queries.
//! Non-global view instances are accessible using the table function `view_instance`. See [ViewInstanceTableFunction](super::view_instance_table_function::ViewInstanceTableFunction).
//!
//! ## log_entries
//!
//! | field        | type                        | description                                               |
//! |------------- |-----------------------------|-----------------------------------------------------------|
//! |process_id    |Utf8                         | unique id of the process, references the processes table  |
//! |exe           |Utf8                         | filename of the process                                   |
//! |username      |Utf8                         | username of the process                                   |
//! |computer      |Utf8                         | computer name of the process                              |
//! |time          |UTC Timestamp (nanoseconds)  | time of the log entry event                               |
//! |target        |Utf8                         | category or module name of the log entry                  |
//! |level         |int32                        | verbosity level (Fatal=1, Error=2, Warning=3, Info=4, Debug=5, Trace=6)|                                           |
//! |msg           |Utf8                         | message                                                   |
//!
//! ### log_entries view instances
//!
//! The implicit use of the `log_entries` table corresponds to the 'global' instance, which contains the log entries of all the processes.
//!
//! Except the 'global' instance, the instance_id refers to any process_id. `view_instance('log_entries', process_id)` contains that process's log. Process-specific views are materialized just-in-time and can provide much better query performance compared to the 'global' instance.
//!
//! ## measures
//!
//! | field        | type                        | description                                               |
//! |------------- |-----------------------------|-----------------------------------------------------------|
//! |process_id    |Utf8                         | unique id of the process, references the processes table  |
//! |exe           |Utf8                         | filename of the process                                   |
//! |username      |Utf8                         | username of the process                                   |
//! |computer      |Utf8                         | computer name of the process                              |
//! |time          |UTC Timestamp (nanoseconds)  | time of the measure event                                 |
//! |target        |Utf8                         | category or module name of the measure                    |
//! |name          |Utf8                         | name of the measure                                       |
//! |unit          |Utf8                         | unit of measure                                           |
//! |value         |Float64                      | value measured                                            |
//!
//!
//! ### measures view instances
//!
//! The implicit use of the `measures` table corresponds to the 'global' instance, which contains the metrics of all the processes.
//!
//! Except the 'global' instance, the instance_id refers to any process_id. `view_instance('measures', process_id)` contains that process's metrics. Process-specific views are materialized just-in-time and can provide much better query performance compared to the 'global' instance.
//!
//! ## thread_spans
//!
//! | field        | type                        | description                                                |
//! |------------- |-----------------------------|------------------------------------------------------------|
//! |id            |Int64                        | span id, unique within this thread                         |
//! |parent        |Int64                        | span id of the calling span                                |
//! |depth         |UInt32                       | call stack depth                                           |
//! |hash          |UInt32                       | identifies a call site (name, filename, line)              |
//! |begin         |UTC Timestamp (nanoseconds)  | when the span started its execution                        |
//! |end           |UTC Timestamp (nanoseconds)  | when the span finished its execution                       |
//! |duration      |Int64 (nanoseconds)          | end-begin                                                  |
//! |name          |Utf8                         | name of the span, usually a function name                  |
//! |target        |Utf8                         | category or module name                                    |
//! |filename      |Utf8                         | name or path of the source file where the span is coded    |
//! |line          |UInt32                       | line number in the file where the span can be found        |
//!
//! ### thread_spans view instances
//!
//! There is no 'global' instance in the 'thread_spans' view set, there is therefore no implicit thread_spans table availble.
//! Users can call the table function `view_instance('thread_spans', stream_id)` to query the spans in the thread associated with the specified stream_id.
//!
//! ## async_events
//!
//! | field        | type                        | description                                               |
//! |------------- |-----------------------------|-----------------------------------------------------------|
//! |stream_id     |Dictionary(Int16, Utf8)      | identifier of the thread stream that emitted the event   |
//! |block_id      |Dictionary(Int16, Utf8)      | unique identifier of the event block                     |
//! |time          |UTC Timestamp (nanoseconds)  | time when the async event occurred                       |
//! |event_type    |Dictionary(Int16, Utf8)      | type of event: "begin" or "end"                          |
//! |span_id       |Int64                        | unique async span identifier                             |
//! |parent_span_id|Int64                        | span id of the parent async span                         |
//! |name          |Dictionary(Int16, Utf8)      | name of the async span, usually a function name          |
//! |filename      |Dictionary(Int16, Utf8)      | name or path of the source file where the span is coded  |
//! |target        |Dictionary(Int16, Utf8)      | category or module name                                   |
//! |line          |UInt32                       | line number in the file where the span can be found      |
//!
//! ### async_events view instances
//!
//! There is no 'global' instance in the 'async_events' view set, there is therefore no implicit async_events table available.
//! Users can call the table function `view_instance('async_events', process_id)` to query the async events in all thread streams associated with the specified process_id.
//! Process-specific views are materialized just-in-time and can provide good query performance.
//!
//! The schema is optimized for high-frequency data. Process information (exe, username, computer, etc.) can be joined when needed:
//! ```sql
//! SELECT ae.*, p.exe, p.username, p.computer
//! FROM view_instance('async_events', process_id) ae
//! JOIN streams s ON ae.stream_id = s.stream_id  
//! JOIN processes p ON s.process_id = p.process_id
//! ```
//!
//! ## processes
//!
//! | field        | type                        | description                                                |
//! |------------- |-----------------------------|------------------------------------------------------------|
//! |process_id    |Utf8                         | process unique id                                          |
//! |exe           |Utf8                         | filename of the process                                    |
//! |username      |Utf8                         | username of the process                                    |
//! |realname      |Utf8                         | real name of the user launching the process                |
//! |computer      |Utf8                         | name of the computer or vm                                 |
//! |distro        |Utf8                         | name of operating system                                   |
//! |cpu_brand     |Utf8                         | identifies the cpu                                         |
//! |tsc frequency |Int64                        | number of ticks per second                                 |
//! |start_time    |UTC Timestamp (nanoseconds)  | when the process started (as reported by the instrumented process) |
//! |start_ticks   |Int64                        | tick count associated with start_time                      |
//! |insert_time   |UTC Timestamp (nanoseconds)  | server-side timestamp when the process metedata was received |
//! |parent_process_id |Utf8                     | unique id of the parent process                            |
//! |properties | Array of {key: utf8, value: utf8} | self-reported metadata by the process                   |
//!
//! There is only one instance in this view set and it is implicitly available.
//!
//! ## streams
//!
//! | field        | type                        | description                                                |
//! |------------- |-----------------------------|------------------------------------------------------------|
//! |stream_id     |Utf8                         | stream unique id                                           |
//! |process_id    |Utf8                         | process unique id                                          |
//! |dependencies_metadata|Binary                | memory layout of the event dependencies                    |
//! |objects_metadata|Binary                     | memory layout of the events                                |
//! |tags          | Array of utf8               | Purpose of the stream, can contain "log", "metrics" or "cpu" |
//! |properties | Array of {key: utf8, value: utf8} | self-reported stream metadata by the process            |
//! |insert_time   |UTC Timestamp (nanoseconds)  | server-side timestamp when the stream metedata was received |
//!
//! There is only one instance in this view set and it is implicitly available.
//!
//! ## blocks
//!
//! | field        | type                        | description                                                |
//! |------------- |-----------------------------|------------------------------------------------------------|
//! |block_id      |Utf8                         | block unique id                                            |
//! |stream_id     |Utf8                         | stream unique id                                           |
//! |process_id    |Utf8                         | process unique id                                          |
//! |begin_time    |UTC Timestamp (nanoseconds)  | system time marking the beginning of this event batch      |
//! |begin_ticks   |Int64                        | tick count associated with begin_time                      |
//! |end_time      |UTC Timestamp (nanoseconds)  | system time marking the ending of this event batch         |
//! |end_ticks     |Int64                        | tick count associated with end_time                        |
//! |nb_objects    |Int32                        | number of events in this batch                             |
//! |object_offset |Int64                        | number of events preceding this batch                      |
//! |payload_size  |Int64                        | number of bytes of the binary payload                      |
//! |insert_time   |UTC Timestamp (nanoseconds)  | server-side timestamp when the block was received          |
//! |streams.dependencies_metadata|Binary        | memory layout of the event dependencies                    |
//! |streams.objects_metadata|Binary             | memory layout of the events                                |
//! |streams.tags  | Array of utf8               | Purpose of the stream, can contain "log", "metrics" or "cpu" |
//! |streams.properties | Array of {key: utf8, value: utf8} | self-reported stream metadata by the process            |
//! |processes.start_time    |UTC Timestamp (nanoseconds)  | when the process started (as reported by the instrumented process) |
//! |processes.start_ticks   |Int64                        | tick count associated with start_time                      |
//! |processes.tsc frequency |Int64                        | number of ticks per second                                 |
//! |processes.exe           |Utf8                         | filename of the process                                    |
//! |processes.username      |Utf8                         | username of the process                                    |
//! |processes.realname      |Utf8                         | real name of the user launching the process                |
//! |processes.computer      |Utf8                         | name of the computer or vm                                 |
//! |processes.distro        |Utf8                         | name of operating system                                   |
//! |processes.cpu_brand     |Utf8                         | identifies the cpu                                         |
//!
//! There is only one instance in this view set and it is implicitly available.
//!
//!
//!
use super::blocks_view::BlocksView;
use super::log_stats_view::make_log_stats_view;
use super::processes_view::make_processes_view;
use super::streams_view::make_streams_view;
use super::{
    async_events_view::AsyncEventsViewMaker, log_view::LogViewMaker,
    metrics_view::MetricsViewMaker, thread_spans_view::ThreadSpansViewMaker, view::View,
};
use anyhow::Result;
use datafusion::arrow::datatypes::Schema;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::fmt::Debug;
use std::{collections::HashMap, sync::Arc};

/// A trait for creating views.
pub trait ViewMaker: Send + Sync + Debug {
    /// Creates a new view with the given instance ID.
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>>;

    /// Returns the schema hash for views created by this maker.
    fn get_schema_hash(&self) -> Vec<u8>;

    /// Returns the schema for views created by this maker.
    fn get_schema(&self) -> Arc<Schema>;
}

/// A factory for creating and managing views.
#[derive(Debug, Clone)]
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

    pub fn get_global_view(&self, view_name: &str) -> Option<Arc<dyn View>> {
        self.global_views
            .iter()
            .find(|v| *(v.get_view_set_name()) == view_name)
            .cloned()
    }

    pub fn add_global_view(&mut self, view: Arc<dyn View>) {
        self.global_views.push(view);
    }

    pub fn add_view_set(&mut self, view_set_name: String, maker: Arc<dyn ViewMaker>) {
        self.view_sets.insert(view_set_name, maker);
    }

    pub fn get_view_sets(&self) -> &HashMap<String, Arc<dyn ViewMaker>> {
        &self.view_sets
    }

    pub fn make_view(&self, view_set_name: &str, view_instance_id: &str) -> Result<Arc<dyn View>> {
        if let Some(maker) = self.view_sets.get(view_set_name) {
            maker.make_view(view_instance_id)
        } else {
            anyhow::bail!("view set {view_set_name} not found");
        }
    }
}

/// Creates the default `ViewFactory` with all built-in views.
pub async fn default_view_factory(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
) -> Result<ViewFactory> {
    let blocks_view = Arc::new(BlocksView::new()?);
    let processes_view = Arc::new(
        make_processes_view(
            runtime.clone(),
            lake.clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await?,
    );
    let streams_view = Arc::new(
        make_streams_view(
            runtime.clone(),
            lake.clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await?,
    );
    let log_view_maker = Arc::new(LogViewMaker {});
    let metrics_view_maker = Arc::new(MetricsViewMaker {});

    // Create base views first
    let global_views = vec![
        log_view_maker.make_view("global")?,
        metrics_view_maker.make_view("global")?,
        processes_view,
        streams_view,
        blocks_view,
    ];
    let mut factory = ViewFactory::new(global_views);
    factory.add_view_set(String::from("log_entries"), log_view_maker.clone());
    factory.add_view_set(String::from("measures"), metrics_view_maker);
    factory.add_view_set(
        String::from("thread_spans"),
        Arc::new(ThreadSpansViewMaker {}),
    );

    // Create the factory as Arc to pass to other view makers
    let factory_arc = Arc::new(factory);

    // Create log_stats view with access to the complete factory (including log_entries)
    let log_stats_view =
        Arc::new(make_log_stats_view(runtime.clone(), lake.clone(), factory_arc.clone()).await?);

    // Clone factory and add log_stats view
    let mut updated_factory = (*factory_arc).clone();
    updated_factory.add_global_view(log_stats_view);

    // Add async_events view maker
    updated_factory.add_view_set(
        String::from("async_events"),
        Arc::new(AsyncEventsViewMaker::new(Arc::new(updated_factory.clone()))),
    );

    Ok(updated_factory)
}
