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
    property_set::{Property, PropertySet, PropertySetId},
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
    enabled_checks: usize,
}

#[derive(Default)]
struct CapturingSink {
    state: Mutex<CapturedState>,
}

fn sorted_pairs(properties: &[(&str, &str)]) -> Pairs {
    let mut pairs: Pairs = properties
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    pairs.sort();
    pairs
}

impl EventSink for CapturingSink {
    fn on_startup(&self, _: Arc<ProcessInfo>) {}
    fn on_shutdown(&self) {}
    fn on_log_enabled(&self, metadata: &LogMetadata) -> bool {
        let mut state = self.state.lock().unwrap();
        state.enabled_checks += 1;
        state.disabled_target.as_deref() != Some(metadata.target)
    }
    fn on_log(
        &self,
        metadata: &LogMetadata,
        properties: &[DynProperty<'_>],
        _: i64,
        args: fmt::Arguments<'_>,
    ) {
        let mut sorted: Pairs = properties
            .iter()
            .map(|p| (p.name().to_owned(), p.value().to_owned()))
            .collect();
        sorted.sort();
        self.state.lock().unwrap().logs.push(LogRecord {
            target: metadata.target.to_owned(),
            msg: args.to_string(),
            properties: sorted,
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

    fn enabled_checks(&self) -> usize {
        self.sink.state.lock().unwrap().enabled_checks
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

    /// (name, target, properties id or none, properties, dynamic dependency bytes of the block) of
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
                out.push(MetricRecord {
                    name: desc.name.to_owned(),
                    target: desc.target.to_owned(),
                    tagged,
                    set_id: props,
                    properties: props
                        .map(|id| sorted_pairs(&block.dyn_deps.set_properties(id)))
                        .unwrap_or_default(),
                    dyn_deps_bytes: block.dyn_deps.len_bytes(),
                });
            }
        }
        out
    }

    /// Event kind, set id, stored message and dynamic dependency bytes of every log event.
    fn log_events(&self) -> Vec<LogEventRecord> {
        flush_log_buffer();
        let state = self.sink.state.lock().unwrap();
        let mut out = vec![];
        for block in &state.log_blocks {
            for evt in block.events.iter() {
                match evt {
                    LogMsgQueueAny::LogStaticStrEvent(_) => out.push(LogEventRecord {
                        kind: "LogStaticStrEvent",
                        tagged: false,
                        set_id: None,
                        msg: None,
                        dyn_deps_bytes: block.dyn_deps.len_bytes(),
                    }),
                    LogMsgQueueAny::LogStringEvent(e) => out.push(LogEventRecord {
                        kind: "LogStringEvent",
                        tagged: false,
                        set_id: None,
                        msg: Some(e.msg.0.clone()),
                        dyn_deps_bytes: block.dyn_deps.len_bytes(),
                    }),
                    LogMsgQueueAny::TaggedLogStaticStrEvent(e) => out.push(LogEventRecord {
                        kind: "TaggedLogStaticStrEvent",
                        tagged: true,
                        set_id: Some(e.properties),
                        msg: None,
                        dyn_deps_bytes: block.dyn_deps.len_bytes(),
                    }),
                    LogMsgQueueAny::TaggedLogString(e) => out.push(LogEventRecord {
                        kind: "TaggedLogString",
                        tagged: true,
                        set_id: Some(e.properties),
                        msg: Some(e.msg.0.clone()),
                        dyn_deps_bytes: block.dyn_deps.len_bytes(),
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
    set_id: Option<PropertySetId>,
    properties: Pairs,
    dyn_deps_bytes: usize,
}

struct LogEventRecord {
    kind: &'static str,
    tagged: bool,
    set_id: Option<PropertySetId>,
    /// `None` for the events that leave the message to the metadata.
    msg: Option<String>,
    dyn_deps_bytes: usize,
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
    let kset = PropertySet::find_or_create(vec![Property::new("k", "set")]);
    let s = "s";
    info!(k = static s, k = "b", "static then literal");
    info!(k = "b", K = static s, "literal then static");
    info!(k = static s, k = x, "static then dynamic");
    info!(k = x, k = static s, "dynamic then static");
    info!(k = static s, ..kset, "static over set");
    info!(k = x, k = "b", ..kset, "spread without static");
    info!(k = x, k = "b", p = static s, ..kset, "spread with static");
    assert_eq!(guard.log_props("static then literal"), pairs(&[("k", "b")]));
    assert_eq!(guard.log_props("literal then static"), pairs(&[("K", "s")]));
    assert_eq!(guard.log_props("static then dynamic"), pairs(&[("k", "x")]));
    assert_eq!(guard.log_props("dynamic then static"), pairs(&[("k", "x")]));
    assert_eq!(guard.log_props("static over set"), pairs(&[("k", "s")]));
    assert_eq!(
        guard.log_props("spread without static"),
        pairs(&[("k", "b")])
    );
    assert_eq!(
        guard.log_props("spread with static"),
        pairs(&[("k", "x"), ("p", "s")])
    );
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
    assert!(logs.iter().all(|l| l.dyn_deps_bytes == 0));
    let literal_ptrs: Vec<_> = tagged.iter().step_by(2).map(|l| l.set_id).collect();
    let spread_ptrs: Vec<_> = tagged.iter().skip(1).step_by(2).map(|l| l.set_id).collect();
    assert!(literal_ptrs.windows(2).all(|w| w[0] == w[1]));
    assert!(
        spread_ptrs
            .iter()
            .all(|p| *p == Some(PropertySetId::interned(set)))
    );
    assert_ne!(literal_ptrs[0], spread_ptrs[0]);

    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 9);
    assert!(metrics.iter().all(|m| m.dyn_deps_bytes == 0));
    for name in ["literal_metric", "literal_metric_float"] {
        let ptrs: Vec<_> = metrics
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.set_id.unwrap())
            .collect();
        assert_eq!(ptrs.len(), 3);
        assert!(ptrs.windows(2).all(|w| w[0] == w[1]));
    }
    assert!(
        metrics
            .iter()
            .filter(|m| m.name == "spread_metric")
            .all(|m| m.set_id == Some(PropertySetId::interned(set)))
    );
}

#[test]
#[serial]
fn dynamic_entries_use_dynamic_deps() {
    let guard = DispatchGuard::new();
    let cid = String::from("c");
    info!(cid = cid, "dynamic");
    imetric!("dynamic", "count", cid = cid, 1);
    let logs = guard.log_events();
    assert!(logs.iter().all(|l| l.tagged && l.dyn_deps_bytes > 0));
    assert!(guard.metrics().iter().all(|m| m.dyn_deps_bytes > 0));
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

#[test]
#[serial]
fn log_enabled_checked_once_per_call() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("kind", "spread")]);
    let i = 7;
    let mut nb_logs = 0;
    let mut check = |label: &str, log: &dyn Fn()| {
        let before = guard.enabled_checks();
        log();
        assert_eq!(guard.enabled_checks() - before, 1, "{label}");
        nb_logs += 1;
        assert_eq!(guard.logs().len(), nb_logs, "{label} was not captured");
    };
    check("plain", &|| info!("m"));
    check("literals", &|| info!(a = "1", "m"));
    check("set only", &|| info!(..set, "m"));
    check("set plus literal", &|| info!(a = "1", ..set, "m"));
    check("dynamic", &|| info!(id = %i, "m"));
    check("literal plus dynamic", &|| info!(a = "1", id = %i, "m"));
    check("set plus dynamic", &|| info!(id = %i, ..set, "m"));
}

#[test]
#[serial]
fn tagged_static_messages_use_the_static_event() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![Property::new("kind", "spread")]);
    info!(a = "1", "static");
    info!(..set, "static");
    let logs = guard.log_events();
    assert_eq!(logs.len(), 2);
    assert!(
        logs.iter()
            .all(|l| l.kind == "TaggedLogStaticStrEvent" && l.msg.is_none())
    );
    assert_eq!(logs[1].set_id, Some(PropertySetId::interned(set)));
    assert!(logs.iter().all(|l| l.dyn_deps_bytes == 0));
    let sink_logs = guard.logs();
    assert_eq!(sink_logs[0], ("static".to_owned(), pairs(&[("a", "1")])));
    assert_eq!(
        sink_logs[1],
        ("static".to_owned(), pairs(&[("kind", "spread")]))
    );
}

#[test]
#[serial]
fn dynamic_static_message_uses_the_static_event() {
    let guard = DispatchGuard::new();
    let x = String::from("x1");
    info!(id = %x, "static");
    let logs = guard.log_events();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].kind, "TaggedLogStaticStrEvent");
    assert!(logs[0].dyn_deps_bytes > 0);
    assert_eq!(guard.log_props("static"), pairs(&[("id", "x1")]));
    let state = guard.sink.state.lock().unwrap();
    let block = &state.log_blocks[0];
    let id = block
        .events
        .iter()
        .find_map(|evt| match evt {
            LogMsgQueueAny::TaggedLogStaticStrEvent(e) => Some(e.properties),
            _ => None,
        })
        .expect("static tagged event");
    assert!(block.dyn_deps.set_properties(id).contains(&("id", "x1")));
}

#[test]
#[serial]
fn tagged_messages_that_differ_from_the_format_string_are_formatted() {
    let guard = DispatchGuard::new();
    let n = 12;
    info!(a = "1", "value {}", 3);
    info!(a = "1", "a {{b}}");
    info!(a = "1", "v {}", n);
    let logs = guard.log_events();
    let msgs: Vec<_> = logs.iter().map(|l| l.msg.clone()).collect();
    assert!(logs.iter().all(|l| l.kind == "TaggedLogString"));
    assert_eq!(
        msgs,
        vec![
            Some("value 3".to_owned()),
            Some("a {b}".to_owned()),
            Some("v 12".to_owned())
        ]
    );
}

#[test]
#[serial]
fn untagged_messages_are_stored_as_printed() {
    let guard = DispatchGuard::new();
    info!("static");
    info!("value {}", 3);
    info!("a {{b}}");
    let logs = guard.log_events();
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0].kind, "LogStaticStrEvent");
    assert_eq!(logs[1].kind, "LogStringEvent");
    assert_eq!(logs[1].msg.as_deref(), Some("value 3"));
    assert_eq!(logs[2].kind, "LogStringEvent");
    assert_eq!(logs[2].msg.as_deref(), Some("a {b}"));
}

fn count_static(counter: &mut u32, s: &'static str) -> &'static str {
    *counter += 1;
    s
}

fn name_of(record: &MetricRecord) -> String {
    record
        .properties
        .iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[test]
#[serial]
fn static_values() {
    let guard = DispatchGuard::new();
    let label: &'static str = "alpha";
    let id = String::from("req-1");
    let set = PropertySet::find_or_create(vec![
        Property::new("region", "eu"),
        Property::new("prefix", "set"),
    ]);
    info!(prefix = static label, "log alone");
    info!(prefix = static label, tier = "ram", "log with literal");
    info!(prefix = static label, request = %id, "log with dynamic");
    info!(prefix = static label, tier = "ram", ..set, "log with set");
    info!(prefix = static label, request = %id, ..set, "log with set and dynamic");
    imetric!("static_alone", "count", prefix = static label, 1_u64);
    imetric!("static_literal", "count", prefix = static label, tier = "ram", 1_u64);
    imetric!("static_dynamic", "count", prefix = static label, request = %id, 1_u64);
    imetric!("static_set", "count", prefix = static label, ..set, 1_u64);
    fmetric!("static_float", "ms", "p-key" = static label, 1.5);
    assert_eq!(guard.log_props("log alone"), pairs(&[("prefix", "alpha")]));
    assert_eq!(
        guard.log_props("log with literal"),
        pairs(&[("prefix", "alpha"), ("tier", "ram")])
    );
    assert_eq!(
        guard.log_props("log with dynamic"),
        pairs(&[("prefix", "alpha"), ("request", "req-1")])
    );
    assert_eq!(
        guard.log_props("log with set"),
        pairs(&[("prefix", "alpha"), ("tier", "ram"), ("region", "eu")])
    );
    assert_eq!(
        guard.log_props("log with set and dynamic"),
        pairs(&[("prefix", "alpha"), ("request", "req-1"), ("region", "eu")])
    );
    assert_eq!(
        metric_props(&guard, "static_alone"),
        pairs(&[("prefix", "alpha")])
    );
    assert_eq!(
        metric_props(&guard, "static_literal"),
        pairs(&[("prefix", "alpha"), ("tier", "ram")])
    );
    assert_eq!(
        metric_props(&guard, "static_dynamic"),
        pairs(&[("prefix", "alpha"), ("request", "req-1")])
    );
    assert_eq!(
        metric_props(&guard, "static_set"),
        pairs(&[("prefix", "alpha"), ("region", "eu")])
    );
    assert_eq!(
        metric_props(&guard, "static_float"),
        pairs(&[("p-key", "alpha")])
    );
}

fn static_metric_once(label: &'static str) {
    imetric!("static_once", "count", prefix = static label, 1_u64);
}

fn static_log_once(label: &'static str) {
    info!(prefix = static label, "static callsite");
}

#[test]
#[serial]
fn static_only_calls_use_interned_sets() {
    let guard = DispatchGuard::new();
    for label in ["a", "b", "a"] {
        static_metric_once(label);
        static_log_once(label);
    }
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 3);
    assert!(metrics.iter().all(|m| m.dyn_deps_bytes == 0));
    let ids: Vec<_> = metrics.iter().map(|m| m.set_id.unwrap()).collect();
    assert_eq!(ids[0], ids[2]);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.iter().all(|id| id.as_interned().is_some()));
    let logs = guard.log_events();
    assert_eq!(logs.len(), 3);
    assert!(logs.iter().all(|l| l.tagged && l.dyn_deps_bytes == 0));
    assert!(
        logs.iter()
            .all(|l| l.set_id.unwrap().as_interned().is_some())
    );
    assert_eq!(logs[0].set_id, logs[2].set_id);
    assert_ne!(logs[0].set_id, logs[1].set_id);
}

#[test]
#[serial]
fn imetric_dyn_takes_runtime_name_and_unit() {
    let guard = DispatchGuard::new();
    let queue = "render";
    let owned = String::from("owned.metric");
    let shared: Arc<str> = Arc::from("shared.metric");
    imetric_dyn!(format!("queue.{queue}.depth"), "count", 1);
    imetric_dyn!(&owned, String::from("bytes"), 2);
    imetric_dyn!(shared.clone(), Arc::<str>::from("ms"), 3);
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 3);
    assert!(metrics.iter().all(|m| m.tagged));
    assert_eq!(
        metrics[0].properties,
        pairs(&[("name", "queue.render.depth"), ("unit", "count")])
    );
    assert_eq!(
        metrics[1].properties,
        pairs(&[("name", "owned.metric"), ("unit", "bytes")])
    );
    assert_eq!(
        metrics[2].properties,
        pairs(&[("name", "shared.metric"), ("unit", "ms")])
    );
}

#[test]
#[serial]
fn static_values_are_evaluated_lazily() {
    let guard = DispatchGuard::new();
    let mut count = 0;
    info!(target: "lazy_static", a = static count_static(&mut count, "x"), "enabled");
    assert_eq!(count, 1);
    guard.disable_target("lazy_static");
    info!(target: "lazy_static", a = static count_static(&mut count, "x"), "disabled");
    info!(
        target: "lazy_static",
        a = static count_static(&mut count, "x"),
        ..PropertySet::empty(),
        "disabled with set"
    );
    assert_eq!(count, 1);
    assert_eq!(guard.logs().len(), 1);
}

#[test]
#[serial]
fn dyn_metrics_with_the_same_name_share_one_set() {
    let guard = DispatchGuard::new();
    for i in 0..3 {
        let name = String::from("same.name");
        imetric_dyn!(&name, "count", i);
    }
    imetric_dyn!(String::from("other.name"), "count", 1);
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 4);
    assert_eq!(metrics[0].set_id, metrics[1].set_id);
    assert_eq!(metrics[1].set_id, metrics[2].set_id);
    assert_ne!(metrics[2].set_id, metrics[3].set_id);
}

#[test]
#[serial]
fn fmetric_dyn_with_target_and_properties() {
    let guard = DispatchGuard::new();
    let name = String::from("http.latency");
    let region = String::from("eu");
    fmetric_dyn!(target: "my_crate::io", &name, "ms", "http.method" = "GET", region = region, 1.5);
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].target, "my_crate::io");
    assert_eq!(
        metrics[0].properties,
        pairs(&[
            ("http.method", "GET"),
            ("region", "eu"),
            ("name", "http.latency"),
            ("unit", "ms")
        ])
    );
}

#[test]
#[serial]
fn dyn_metric_name_wins_over_a_spread_set() {
    let guard = DispatchGuard::new();
    let set = PropertySet::find_or_create(vec![
        Property::new("name", "from_set"),
        Property::new("unit", "from_set"),
        Property::new("region", "eu"),
    ]);
    imetric_dyn!("runtime.name", "count", ..set, 1);
    let metrics = guard.metrics();
    assert_eq!(
        metrics[0].properties,
        pairs(&[
            ("name", "runtime.name"),
            ("unit", "count"),
            ("region", "eu")
        ])
    );
    assert_eq!(name_of(&metrics[0]), "runtime.name");
}

#[test]
#[serial]
fn static_metrics_stay_untagged_next_to_dyn_ones() {
    let guard = DispatchGuard::new();
    imetric!("static_int", "count", 1);
    fmetric!("static_float", "count", 1.0);
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 2);
    assert!(metrics.iter().all(|m| !m.tagged && m.dyn_deps_bytes == 0));
}

#[test]
#[serial]
fn dyn_metric_with_a_static_value() {
    let guard = DispatchGuard::new();
    let label: &'static str = "alpha";
    let name = String::from("dyn.static");
    imetric_dyn!(&name, "count", prefix = static label, 1);
    fmetric_dyn!(&name, "ms", prefix = static label, "p-key" = "lit", 1.5);
    let metrics = guard.metrics();
    assert_eq!(metrics.len(), 2);
    assert_eq!(
        metrics[0].properties,
        pairs(&[
            ("prefix", "alpha"),
            ("name", "dyn.static"),
            ("unit", "count")
        ])
    );
    assert_eq!(
        metrics[1].properties,
        pairs(&[
            ("prefix", "alpha"),
            ("p-key", "lit"),
            ("name", "dyn.static"),
            ("unit", "ms")
        ])
    );
}
