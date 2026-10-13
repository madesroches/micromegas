use micromegas_telemetry_sink::tracing_interop::TracingCaptureLayer;
use micromegas_tracing::{
    dispatch::flush_log_buffer, levels::LevelFilter, logs::LogMsgQueueAny,
    test_utils::init_in_memory_tracing,
};
use micromegas_transit::{DynString, HeterogeneousQueue};
use serial_test::serial;
use tracing_subscriber::prelude::*;

type Captured = (String, Vec<(String, String)>);

fn captured_interop_events(
    guard: &micromegas_tracing::test_utils::InMemoryTracingGuard,
) -> (Vec<Captured>, usize) {
    let state = guard.sink.state.lock().unwrap();
    let mut tagged = vec![];
    let mut untagged = 0;
    for block in &state.log_blocks {
        for evt in block.events.iter() {
            match evt {
                LogMsgQueueAny::TaggedLogInteropEvent(e) => {
                    let DynString(msg) = &e.msg;
                    let props = unsafe { &*e.properties }
                        .get_properties()
                        .iter()
                        .map(|p| (p.name.as_str().to_owned(), p.value.as_str().to_owned()))
                        .collect();
                    tagged.push((msg.clone(), props));
                }
                LogMsgQueueAny::LogStringInteropEvent(_) => untagged += 1,
                _ => {}
            }
        }
    }
    (tagged, untagged)
}

#[test]
#[serial]
fn event_fields_become_properties() {
    let guard = init_in_memory_tracing();
    let subscriber = tracing_subscriber::registry().with(TracingCaptureLayer {
        max_level: LevelFilter::Trace,
    });
    tracing::subscriber::with_default(subscriber, || {
        let id = String::from("abc-123");
        tracing::info!(correlation_id = %id, n = 3, user = "bob", log.module_path = "ignored", "hello");
        tracing::info!("no fields");
    });
    flush_log_buffer();

    let (tagged, untagged) = captured_interop_events(&guard);
    assert_eq!(untagged, 1);
    assert_eq!(tagged.len(), 1);
    let (msg, props) = &tagged[0];
    assert!(msg.contains("correlation_id=abc-123"), "{msg}");
    assert!(msg.contains("n=3"), "{msg}");
    assert!(msg.contains("user=\"bob\""), "{msg}");
    assert!(msg.contains("hello"), "{msg}");
    let get = |name: &str| -> Vec<&str> {
        props
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    };
    assert_eq!(get("correlation_id"), vec!["abc-123"]);
    assert_eq!(get("n"), vec!["3"]);
    assert_eq!(get("user"), vec!["bob"]);
    assert!(get("message").is_empty());
    assert!(props.iter().all(|(k, _)| !k.starts_with("log.")));
}
