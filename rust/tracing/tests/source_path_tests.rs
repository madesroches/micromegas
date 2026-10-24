use micromegas_tracing::{
    dyn_deps::DynDeps,
    event::ExtractDeps,
    levels::Level,
    logs::{
        FILTER_LEVEL_UNSET_VALUE, LogDepsQueue, LogDepsQueueAny, LogMetadata, LogMsgQueue,
        LogStaticStrEvent,
    },
    metrics::{
        IntegerMetricEvent, MetricsDepsQueue, MetricsDepsQueueAny, MetricsMsgQueue,
        StaticMetricMetadata,
    },
    prelude::*,
    source_path::trim_source_path,
    spans::{
        BeginThreadNamedSpanEvent, BeginThreadSpanEvent, SpanLocation, SpanMetadata,
        ThreadDepsQueue, ThreadDepsQueueAny, ThreadEventQueue,
    },
};
use micromegas_transit::HeterogeneousQueue;
use std::sync::atomic::AtomicU32;

#[test]
fn trims_paths() {
    let cases = [
        (
            "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.40.0/src/runtime/task.rs",
            "tokio-1.40.0/src/runtime/task.rs",
        ),
        (
            "/home/u/.cargo/registry/src/github.com-1ecc6299db9ec823/tokio-1.40.0/src/runtime/task.rs",
            "tokio-1.40.0/src/runtime/task.rs",
        ),
        (
            r"C:\Users\u\.cargo\registry\src\index.crates.io-x\tokio-1.40.0\src\a.rs",
            r"tokio-1.40.0\src\a.rs",
        ),
        (
            "/home/u/.cargo/git/checkouts/micromegas-1a2b/5e6f7a8/rust/tracing/src/lib.rs",
            "micromegas-1a2b/5e6f7a8/rust/tracing/src/lib.rs",
        ),
        ("/home/u/dep/src/lib.rs", "lib.rs"),
        (r"D:\dep\src\lib.rs", "lib.rs"),
        ("src/lib.rs", "src/lib.rs"),
        ("rust/registry/src/x/y.rs", "rust/registry/src/x/y.rs"),
        ("", ""),
        ("/x/myregistry/src/idx/tokio/src/lib.rs", "lib.rs"),
        // a marker with nothing usable after it falls back to the file name
        ("/home/u/.cargo/registry/src/index-x", "index-x"),
        ("/home/u/.cargo/git/checkouts/", ""),
    ];
    for (input, expected) in cases {
        assert_eq!(trim_source_path(input), expected, "input: {input}");
    }
}

#[test]
fn result_is_a_suffix_of_the_input() {
    let input = "/home/u/.cargo/registry/src/index.crates.io-x/tokio-1.40.0/src/a.rs";
    let trimmed = trim_source_path(input);
    assert_eq!(
        trimmed.as_bytes().as_ptr_range().end,
        input.as_bytes().as_ptr_range().end,
        "the trimmed path must end where the input ends"
    );
}

const REGISTRY_FILE: &str = "/home/u/.cargo/registry/src/index.crates.io-x/tokio-1.40.0/src/a.rs";

fn trimmed_ptr() -> *const u8 {
    trim_source_path(REGISTRY_FILE).as_ptr()
}

fn trimmed_len() -> u32 {
    trim_source_path(REGISTRY_FILE).len() as u32
}

static LOG_DESC: LogMetadata = LogMetadata {
    level: Level::Info,
    level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
    fmt_str: "",
    target: "source_path_tests",
    module_path: "source_path_tests",
    file: REGISTRY_FILE,
    line: 1,
};

static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Max,
    name: "source_path_metric",
    unit: "count",
    target: "source_path_tests",
    file: REGISTRY_FILE,
    line: 1,
};

static SPAN_DESC: SpanMetadata = SpanMetadata {
    name: "source_path_span",
    location: SpanLocation {
        lod: Verbosity::Max,
        target: "source_path_tests",
        module_path: "source_path_tests",
        file: REGISTRY_FILE,
        line: 1,
    },
};

static SPAN_LOCATION: SpanLocation = SpanLocation {
    lod: Verbosity::Max,
    target: "source_path_tests",
    module_path: "source_path_tests",
    file: REGISTRY_FILE,
    line: 2,
};

#[test]
fn log_extraction_uses_the_trimmed_path() {
    let mut events = LogMsgQueue::new(4096);
    events.push(LogStaticStrEvent {
        desc: &LOG_DESC,
        time: 0,
    });
    let deps = events.extract(&DynDeps::<LogDepsQueue>::new());
    let mut record_file = None;
    let mut dep_found = false;
    for x in deps.iter() {
        match x {
            LogDepsQueueAny::LogMetadataRecord(rec) => record_file = Some(rec.file),
            LogDepsQueueAny::Utf8StaticStringDependency(dep) => {
                dep_found |= dep.ptr == trimmed_ptr() && dep.len == trimmed_len();
            }
            _ => {}
        }
    }
    assert_eq!(record_file, Some(trimmed_ptr()));
    assert!(dep_found, "no string dependency for the trimmed path");
}

#[test]
fn metric_extraction_uses_the_trimmed_path() {
    let mut events = MetricsMsgQueue::new(4096);
    events.push(IntegerMetricEvent {
        desc: &METRIC_DESC,
        value: 1,
        time: 0,
    });
    let deps = events.extract(&DynDeps::<MetricsDepsQueue>::new());
    let mut record_file = None;
    let mut dep_found = false;
    for x in deps.iter() {
        match x {
            MetricsDepsQueueAny::MetricMetadataDependency(rec) => record_file = Some(rec.file),
            MetricsDepsQueueAny::Utf8StaticStringDependency(dep) => {
                dep_found |= dep.ptr == trimmed_ptr() && dep.len == trimmed_len();
            }
            _ => {}
        }
    }
    assert_eq!(record_file, Some(trimmed_ptr()));
    assert!(dep_found, "no string dependency for the trimmed path");
}

#[test]
fn thread_extraction_uses_the_trimmed_path() {
    let mut events = ThreadEventQueue::new(4096);
    events.push(BeginThreadSpanEvent {
        thread_span_desc: &SPAN_DESC,
        time: 0,
    });
    events.push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION,
        name: "named".into(),
        time: 0,
    });
    let deps = events.extract(&DynDeps::<ThreadDepsQueue>::new());
    let mut span_file = None;
    let mut location_file = None;
    let mut dep_found = false;
    for x in deps.iter() {
        match x {
            ThreadDepsQueueAny::SpanRecord(rec) => span_file = Some(rec.file),
            ThreadDepsQueueAny::SpanLocationRecord(rec) => location_file = Some(rec.file),
            ThreadDepsQueueAny::Utf8StaticStringDependency(dep) => {
                dep_found |= dep.ptr == trimmed_ptr() && dep.len == trimmed_len();
            }
            _ => {}
        }
    }
    assert_eq!(span_file, Some(trimmed_ptr()));
    assert_eq!(location_file, Some(trimmed_ptr()));
    assert!(dep_found, "no string dependency for the trimmed path");
}
