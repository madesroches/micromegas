use micromegas_tracing::{
    dispatch::{
        flush_log_buffer, flush_metrics_buffer, force_uninit, init_event_dispatch,
        shutdown_dispatch,
    },
    event::EventSink,
    levels::{Level, LevelFilter, set_max_level},
    logs::{LogBlock, LogMetadata, LogMsgQueueAny, LogStream},
    metrics::{MetricsBlock, MetricsMsgQueueAny, MetricsStream},
    prelude::*,
    process_info::ProcessInfo,
    property_set::{Property, PropertySet},
    spans::{ThreadBlock, ThreadStream},
};
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

type Pairs = Vec<(String, String)>;

#[derive(Debug)]
struct LogRecord {
    target: String,
    msg: String,
    properties: Pairs,
}

#[derive(Default)]
struct CapturedState {
    logs: Vec<LogRecord>,
    log_blocks: Vec<Arc<LogBlock>>,
    metrics_blocks: Vec<Arc<MetricsBlock>>,
    disabled_target: Option<String>,
}

#[derive(Default)]
struct CapturingSink {
    state: Mutex<CapturedState>,
}

fn sorted_pairs(properties: &[Property]) -> Pairs {
    let mut pairs: Pairs = properties
        .iter()
        .map(|p| (p.name.as_str().to_owned(), p.value.as_str().to_owned()))
        .collect();
    pairs.sort();
    pairs
}

impl EventSink for CapturingSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool {
        self.state.lock().unwrap().disabled_target.as_deref() != Some(metadata.target)
    }
    fn on_log(
        &self,
        metadata: &LogMetadata,
        properties: &[Property],
        _: i64,
        args: fmt::Arguments<'_>,
    ) {
        self.state.lock().unwrap().logs.push(LogRecord {
            target: metadata.target.to_owned(),
            msg: args.to_string(),
            properties: sorted_pairs(properties),
        });
    }
    fn on_init_log_stream(&self, _: &LogStream) {}
    fn on_process_log_block(&self, block: Arc<LogBlock>) {
        self.state.lock().unwrap().log_blocks.push(block);
    }
    fn on_init_metrics_stream(&self, _: &MetricsStream) {}
    fn on_process_metrics_block(&self, block: Arc<MetricsBlock>) {
        self.state.lock().unwrap().metrics_blocks.push(block);
    }
    fn on_init_thread_stream(&self, _: &ThreadStream) {}
    fn on_process_thread_block(&self, _: Arc<ThreadBlock>) {}
    fn is_busy(&self) -> bool {
        false
    }
}

struct DispatchGuard {
    sink: Arc<CapturingSink>,
}

impl DispatchGuard {
    fn new() -> Self {
        let sink = Arc::new(CapturingSink::default());
        init_event_dispatch(
            64 * 1024,
            64 * 1024,
            1024,
            sink.clone(),
            HashMap::new(),
            false,
        )
        .unwrap();
        set_max_level(LevelFilter::Trace);
        Self { sink }
    }

    fn disable_target(&self, target: &str) {
        self.sink.state.lock().unwrap().disabled_target = Some(target.to_owned());
    }

    fn logs(&self) -> Vec<(String, Pairs)> {
        self.sink
            .state
            .lock()
            .unwrap()
            .logs
            .iter()
            .map(|l| (l.msg.clone(), l.properties.clone()))
            .collect()
    }

    /// Properties of the single entry logged with this message.
    fn log_props(&self, msg: &str) -> Pairs {
        let state = self.sink.state.lock().unwrap();
        let mut matches = state.logs.iter().filter(|l| l.msg == msg);
        let record = matches.next().unwrap_or_else(|| panic!("no log {msg:?}"));
        assert!(matches.next().is_none(), "several logs {msg:?}");
        record.properties.clone()
    }

    fn log_target(&self, msg: &str) -> String {
        let state = self.sink.state.lock().unwrap();
        state
            .logs
            .iter()
            .find(|l| l.msg == msg)
            .unwrap_or_else(|| panic!("no log {msg:?}"))
            .target
            .clone()
    }

    /// (name, target, properties pointer or null, properties, arena bytes of the block) of
    /// every metric event.
    fn metrics(&self) -> Vec<MetricRecord> {
        flush_metrics_buffer();
        let state = self.sink.state.lock().unwrap();
        let mut out = vec![];
        for block in &state.metrics_blocks {
            for evt in block.events.iter() {
                let (desc, props, tagged) = match evt {
                    MetricsMsgQueueAny::IntegerMetricEvent(e) => (e.desc, None, false),
                    MetricsMsgQueueAny::FloatMetricEvent(e) => (e.desc, None, false),
                    MetricsMsgQueueAny::TaggedIntegerMetricEvent(e) => {
                        (e.desc, Some(e.properties), true)
                    }
                    MetricsMsgQueueAny::TaggedFloatMetricEvent(e) => {
                        (e.desc, Some(e.properties), true)
                    }
                };
                // SAFETY: the set is interned or owned by the block, which `state` keeps alive.
                let set = props.map(|p| unsafe { &*p });
                out.push(MetricRecord {
                    name: desc.name.to_owned(),
                    target: desc.target.to_owned(),
                    tagged,
                    set_ptr: props.map(|p| p as usize),
                    properties: set
                        .map(|s| sorted_pairs(s.get_properties()))
                        .unwrap_or_default(),
                    arena_bytes: block.properties.len_bytes(),
                });
            }
        }
        out
    }

    /// (kind, set pointer, properties) of every log event, and the arena bytes of its block.
    fn log_events(&self) -> Vec<LogEventRecord> {
        flush_log_buffer();
        let state = self.sink.state.lock().unwrap();
        let mut out = vec![];
        for block in &state.log_blocks {
            for evt in block.events.iter() {
                match evt {
                    LogMsgQueueAny::LogStaticStrEvent(_) | LogMsgQueueAny::LogStringEvent(_) => out
                        .push(LogEventRecord {
                            tagged: false,
                            set_ptr: 0,
                            arena_bytes: block.properties.len_bytes(),
                        }),
                    LogMsgQueueAny::TaggedLogString(e) => out.push(LogEventRecord {
                        tagged: true,
                        set_ptr: e.properties as usize,
                        arena_bytes: block.properties.len_bytes(),
                    }),
                    _ => {}
                }
            }
        }
        out
    }
}

struct MetricRecord {
    name: String,
    target: String,
    tagged: bool,
    set_ptr: Option<usize>,
    properties: Pairs,
    arena_bytes: usize,
}

struct LogEventRecord {
    tagged: bool,
    set_ptr: usize,
    arena_bytes: usize,
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        shutdown_dispatch();
        unsafe { force_uninit() };
    }
}

fn pairs(entries: &[(&str, &str)]) -> Pairs {
    let mut v: Pairs = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    v.sort();
    v
}

fn metric_props(guard: &DispatchGuard, name: &str) -> Pairs {
    guard
        .metrics()
        .into_iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no metric {name:?}"))
        .properties
}

struct Display42;
impl fmt::Display for Display42 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "forty-two")
    }
}
#[derive(Debug)]
struct Debug42;

#[test]
#[serial]
fn log_key_value_forms() {
    let guard = DispatchGuard::new();
    let owned = String::from("owned");
    let n = 7;
    info!(lit = "a", num = 3, flag = true, "literals");
    info!(who = owned, "expr");
    info!(shown = %Display42, "display");
    info!(debugged = ?Debug42, "debug");
    info!("http.method" = "GET", "string key");
    info!(neg = -1, signed = -2.5, "negative literals");
    info!(shown = %n, "display int {}", n);
    assert_eq!(
        guard.log_props("literals"),
        pairs(&[("lit", "a"), ("num", "3"), ("flag", "true")])
    );
    assert_eq!(guard.log_props("expr"), pairs(&[("who", "owned")]));
    assert_eq!(guard.log_props("display"), pairs(&[("shown", "forty-two")]));
    assert_eq!(guard.log_props("debug"), pairs(&[("debugged", "Debug42")]));
    assert_eq!(
        guard.log_props("string key"),
        pairs(&[("http.method", "GET")])
    );
    assert_eq!(
        guard.log_props("negative literals"),
        pairs(&[("neg", "-1"), ("signed", "-2.5")])
    );
    assert_eq!(guard.log_props("display int 7"), pairs(&[("shown", "7")]));
}

#[test]
#[serial]
fn bare_values_of_string_types() {
    let guard = DispatchGuard::new();
    let s = String::from("string");
    let r: &str = "str";
    let a = Arc::new(String::from("arc"));
    let a_str: Arc<str> = Arc::from("arcstr");
    let c: Cow<'_, str> = Cow::Borrowed("cow");
    let b: Box<str> = "box".into();
    info!(
        s = s,
        r = r,
        a = a,
        a_str = a_str,
        c = c,
        b = b,
        rs = &s,
        "bare"
    );
    assert_eq!(
        guard.log_props("bare"),
        pairs(&[
            ("s", "string"),
            ("r", "str"),
            ("a", "arc"),
            ("a_str", "arcstr"),
            ("c", "cow"),
            ("b", "box"),
            ("rs", "string"),
        ])
    );
    imetric!("bare", "count", s = s, a = a, r = r, 1);
    assert_eq!(
        metric_props(&guard, "bare"),
        pairs(&[("s", "string"), ("a", "arc"), ("r", "str")])
    );
}

#[test]
#[serial]
fn spread_sets() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![
        Property::new("region", "eu"),
        Property::new("kind", "set"),
    ]);
    let cid = String::from("c0ffee");
    info!(..set, "set only");
    info!(kind = "pair", cid = cid, ..set, "pairs override the set");
    info!(Kind = "mixed", ..set, "case insensitive override");
    assert_eq!(
        guard.log_props("set only"),
        pairs(&[("region", "eu"), ("kind", "set")])
    );
    assert_eq!(
        guard.log_props("pairs override the set"),
        pairs(&[("region", "eu"), ("kind", "pair"), ("cid", "c0ffee")])
    );
    assert_eq!(
        guard.log_props("case insensitive override"),
        pairs(&[("region", "eu"), ("Kind", "mixed")])
    );
    imetric!("spread", "count", ..set, 1);
    imetric!("spread_pair", "count", kind = "pair", ..set, 1_u64);
    fmetric!("spread_float", "count", cid = cid, ..set, 1.0);
    imetric!("spread_trailing_comma", "count", ..set, 1,);
    assert_eq!(
        metric_props(&guard, "spread"),
        pairs(&[("region", "eu"), ("kind", "set")])
    );
    assert_eq!(
        metric_props(&guard, "spread_trailing_comma"),
        pairs(&[("region", "eu"), ("kind", "set")])
    );
    assert_eq!(
        metric_props(&guard, "spread_pair"),
        pairs(&[("region", "eu"), ("kind", "pair")])
    );
    assert_eq!(
        metric_props(&guard, "spread_float"),
        pairs(&[("region", "eu"), ("kind", "set"), ("cid", "c0ffee")])
    );
}

#[test]
#[serial]
fn reserved_names_pass_through_a_spread() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("name", "road_width")]);
    imetric!("", "", ..set, 2);
    assert_eq!(metric_props(&guard, ""), pairs(&[("name", "road_width")]));
}

#[test]
#[serial]
fn duplicate_keys() {
    let guard = DispatchGuard::new();
    let x = String::from("x");
    let set = PropertySet::empty();
    info!(k = "a", k = "b", "last literal wins");
    info!(k = "a", K = "b", "case insensitive literal");
    info!(k = x, k = "a", "dynamic first, literal second");
    info!(k = "a", k = x, "literal first, dynamic second");
    info!(k = x, K = %"y", "last dynamic wins");
    info!(k = "a", k = "b", ..set, "spread keeps order");
    info!(k = x, k = "b", ..set, "spread keeps order, dynamic first");
    assert_eq!(guard.log_props("last literal wins"), pairs(&[("k", "b")]));
    assert_eq!(
        guard.log_props("case insensitive literal"),
        pairs(&[("K", "b")])
    );
    assert_eq!(
        guard.log_props("dynamic first, literal second"),
        pairs(&[("k", "x")])
    );
    assert_eq!(
        guard.log_props("literal first, dynamic second"),
        pairs(&[("k", "x")])
    );
    assert_eq!(guard.log_props("last dynamic wins"), pairs(&[("K", "y")]));
    assert_eq!(guard.log_props("spread keeps order"), pairs(&[("k", "b")]));
    assert_eq!(
        guard.log_props("spread keeps order, dynamic first"),
        pairs(&[("k", "b")])
    );
    imetric!("dups", "count", k = "a", K = "b", 1);
    assert_eq!(metric_props(&guard, "dups"), pairs(&[("K", "b")]));
}

#[test]
#[serial]
fn trailing_commas_and_named_format_args() {
    let guard = DispatchGuard::new();
    let cid = String::from("c");
    info!(a = "1", cid = cid, "with trailing comma {}", 1,);
    info!(a = "1", "named {x} {y}", x = 1, y = 2);
    imetric!("comma", "count", a = "1", cid = cid, 1,);
    fmetric!("comma_float", "count", a = "1", 1.0,);
    imetric!("comma_plain", "count", 1,);
    assert_eq!(
        guard.log_props("with trailing comma 1"),
        pairs(&[("a", "1"), ("cid", "c")])
    );
    assert_eq!(guard.log_props("named 1 2"), pairs(&[("a", "1")]));
    assert_eq!(
        metric_props(&guard, "comma"),
        pairs(&[("a", "1"), ("cid", "c")])
    );
    assert_eq!(metric_props(&guard, "comma_float"), pairs(&[("a", "1")]));
    assert!(
        !guard
            .metrics()
            .iter()
            .find(|m| m.name == "comma_plain")
            .unwrap()
            .tagged
    );
}

#[test]
#[serial]
fn targets() {
    let guard = DispatchGuard::new();
    let cid = String::from("c");
    info!(target: "custom", k = "v", cid = cid, "targeted");
    info!(target: "custom", "targeted plain");
    imetric!(target: "custom_metric", "t1", "count", k = "v", 1);
    fmetric!(target: "custom_metric", "t2", "count", cid = cid, 1.0);
    imetric!(target: "custom_metric", "t3", "count", 1);
    imetric!("t4", "count", k = "v", 1);
    assert_eq!(guard.log_target("targeted"), "custom");
    assert_eq!(
        guard.log_props("targeted"),
        pairs(&[("k", "v"), ("cid", "c")])
    );
    assert_eq!(guard.log_target("targeted plain"), "custom");
    let metrics = guard.metrics();
    let target_of = |name: &str| {
        metrics
            .iter()
            .find(|m| m.name == name)
            .unwrap()
            .target
            .clone()
    };
    assert_eq!(target_of("t1"), "custom_metric");
    assert_eq!(target_of("t2"), "custom_metric");
    assert_eq!(target_of("t3"), "custom_metric");
    assert_eq!(target_of("t4"), module_path!());
    let t1 = metrics.iter().find(|m| m.name == "t1").unwrap();
    assert_eq!(t1.properties, pairs(&[("k", "v")]));
}

#[test]
#[serial]
fn negative_metric_values() {
    let guard = DispatchGuard::new();
    let x = 2.5_f64;
    let a = 1.0_f64;
    let b = 2.0_f64;
    fmetric!("neg", "u", -x);
    fmetric!("neg_kv", "u", k = "v", -x);
    fmetric!("neg_expr", "u", k = "v", -a + b);
    fmetric!("neg_dyn", "u", k = %-x, -x);
    fmetric!("neg_lit", "u", k = -1, 1.0);
    assert_eq!(metric_props(&guard, "neg_kv"), pairs(&[("k", "v")]));
    assert_eq!(metric_props(&guard, "neg_dyn"), pairs(&[("k", "-2.5")]));
    assert_eq!(metric_props(&guard, "neg_lit"), pairs(&[("k", "-1")]));
}

#[test]
#[serial]
fn no_property_calls_stay_untagged() {
    let guard = DispatchGuard::new();
    info!("plain");
    info!("plain {}", 1);
    imetric!("plain", "count", 1);
    fmetric!("plain_float", "count", 1.0);
    let logs = guard.log_events();
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().all(|l| !l.tagged));
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 2);
    assert!(metrics.iter().all(|m| !m.tagged));
}

fn log_literals_once() {
    info!(region = "eu", n = 3, "literal callsite");
}

fn metric_literals_once() {
    imetric!("literal_metric", "count", region = "eu", 1);
    fmetric!("literal_metric_float", "count", region = "eu", 1.0);
}

fn log_spread_once(set: &'static PropertySet) {
    info!(..set, "spread callsite");
}

fn metric_spread_once(set: &'static PropertySet) {
    imetric!("spread_metric", "count", ..set, 1);
}

#[test]
#[serial]
fn literal_sets_are_interned_per_callsite() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("kind", "spread")]);
    for _ in 0..3 {
        log_literals_once();
        log_spread_once(set);
        metric_literals_once();
        metric_spread_once(set);
        flush_log_buffer();
        flush_metrics_buffer();
    }
    let logs = guard.log_events();
    let tagged: Vec<_> = logs.iter().filter(|l| l.tagged).collect();
    assert_eq!(tagged.len(), 6);
    assert!(logs.iter().all(|l| l.arena_bytes == 0));
    let literal_ptrs: Vec<_> = tagged.iter().step_by(2).map(|l| l.set_ptr).collect();
    let spread_ptrs: Vec<_> = tagged
        .iter()
        .skip(1)
        .step_by(2)
        .map(|l| l.set_ptr)
        .collect();
    assert!(literal_ptrs.windows(2).all(|w| w[0] == w[1]));
    assert!(
        spread_ptrs
            .iter()
            .all(|p| *p == set as *const PropertySet as usize)
    );
    assert_ne!(literal_ptrs[0], spread_ptrs[0]);

    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 9);
    assert!(metrics.iter().all(|m| m.arena_bytes == 0));
    for name in ["literal_metric", "literal_metric_float"] {
        let ptrs: Vec<_> = metrics
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.set_ptr.unwrap())
            .collect();
        assert_eq!(ptrs.len(), 3);
        assert!(ptrs.windows(2).all(|w| w[0] == w[1]));
    }
    assert!(
        metrics
            .iter()
            .filter(|m| m.name == "spread_metric")
            .all(|m| m.set_ptr == Some(set as *const PropertySet as usize))
    );
}

#[test]
#[serial]
fn dynamic_entries_use_the_arena() {
    let guard = DispatchGuard::new();
    let cid = String::from("c");
    info!(cid = cid, "dynamic");
    imetric!("dynamic", "count", cid = cid, 1);
    let logs = guard.log_events();
    assert!(logs.iter().all(|l| l.tagged && l.arena_bytes > 0));
    assert!(guard.metrics().iter().all(|m| m.arena_bytes > 0));
}

fn count_and_return<'a>(counter: &mut u32, s: &'a str) -> &'a str {
    *counter += 1;
    s
}

#[test]
#[serial]
fn dynamic_values_are_evaluated_lazily() {
    let guard = DispatchGuard::new();
    let mut bare = 0;
    let mut display = 0;
    let mut debug = 0;
    info!(target: "lazy", a = count_and_return(&mut bare, "x"), "enabled bare");
    info!(target: "lazy", a = %count_and_return(&mut display, "x"), "enabled display");
    info!(target: "lazy", a = ?count_and_return(&mut debug, "x"), "enabled debug");
    assert_eq!((bare, display, debug), (1, 1, 1));

    guard.disable_target("lazy");
    info!(target: "lazy", a = count_and_return(&mut bare, "x"), "disabled bare");
    info!(target: "lazy", a = %count_and_return(&mut display, "x"), "disabled display");
    info!(target: "lazy", a = ?count_and_return(&mut debug, "x"), "disabled debug");
    assert_eq!((bare, display, debug), (1, 1, 1));
    assert_eq!(guard.logs().len(), 3);

    // above the max level: sanity check only, the level check already short-circuits it
    set_max_level(LevelFilter::Error);
    log!(
        Level::Info,
        a = count_and_return(&mut bare, "x"),
        "too verbose"
    );
    assert_eq!(bare, 1);
}
