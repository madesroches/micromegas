use micromegas_tracing::levels::{Level, LevelFilter};
use micromegas_tracing::logs::{FILTER_LEVEL_UNSET_VALUE, FilterState, LogMetadata};
use std::thread;

#[test]
fn test_filter_levels() {
    static METADATA: LogMetadata = LogMetadata {
        level: Level::Trace,
        level_filter: std::sync::atomic::AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "$crate::__first_arg!($($arg)+)",
        target: module_path!(),
        module_path: module_path!(),
        file: file!(),
        line: line!(),
    };
    assert_eq!(METADATA.level_filter(1), FilterState::Outdated);
    METADATA.set_level_filter(1, Some(LevelFilter::Trace));
    assert_eq!(
        METADATA.level_filter(1),
        FilterState::Set(LevelFilter::Trace)
    );
    METADATA.set_level_filter(1, Some(LevelFilter::Debug));
    assert_eq!(
        METADATA.level_filter(1),
        FilterState::Set(LevelFilter::Trace)
    );
    METADATA.set_level_filter(1, None);
    assert_eq!(
        METADATA.level_filter(1),
        FilterState::Set(LevelFilter::Trace)
    );
    METADATA.set_level_filter(2, Some(LevelFilter::Debug));
    assert_eq!(
        METADATA.level_filter(1),
        FilterState::Set(LevelFilter::Debug)
    );
    assert_eq!(
        METADATA.level_filter(2),
        FilterState::Set(LevelFilter::Debug)
    );
    METADATA.set_level_filter(1, Some(LevelFilter::Info));
    assert_eq!(
        METADATA.level_filter(2),
        FilterState::Set(LevelFilter::Debug)
    );
    assert_eq!(METADATA.level_filter(3), FilterState::Outdated);
    METADATA.set_level_filter(3, None);
    assert_eq!(METADATA.level_filter(3), FilterState::NotSet);
    METADATA.set_level_filter(3, Some(LevelFilter::Warn));
    assert_eq!(METADATA.level_filter(3), FilterState::NotSet);

    let mut threads = Vec::new();
    for _ in 0..1 {
        threads.push(thread::spawn(move || {
            for i in 0..1024 {
                let filter = match i % 6 {
                    0 => LevelFilter::Off,
                    1 => LevelFilter::Error,
                    2 => LevelFilter::Warn,
                    3 => LevelFilter::Info,
                    4 => LevelFilter::Debug,
                    5 => LevelFilter::Trace,
                    _ => unreachable!(),
                };

                METADATA.set_level_filter(i, Some(filter));
            }
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(
        METADATA.level_filter(1023),
        FilterState::Set(LevelFilter::Info)
    );
    assert_eq!(METADATA.level_filter(1024), FilterState::Outdated);
}
