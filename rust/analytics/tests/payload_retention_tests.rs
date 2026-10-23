use chrono::{DateTime, Days, TimeZone, Utc};
use micromegas_analytics::delete::{BlockDeletePass, PayloadRetention, RetentionHorizon};
use std::collections::BTreeMap;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
}

fn ago(days: u64) -> DateTime<Utc> {
    now().checked_sub_days(Days::new(days)).unwrap()
}

fn policy(default_days: u32, overrides: &[(&str, RetentionHorizon)]) -> PayloadRetention {
    PayloadRetention {
        default_days,
        by_stream_tag: overrides
            .iter()
            .map(|(t, h)| (t.to_string(), *h))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn tags(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_overrides_is_a_single_default_pass() {
    let passes = policy(90, &[]).block_delete_passes(now()).unwrap();
    assert_eq!(
        passes,
        vec![BlockDeletePass {
            cutoff: ago(90),
            include_tags: None,
            exclude_tags: vec![],
        }]
    );
}

#[test]
fn forever_tag_only_excludes_from_default() {
    let passes = policy(90, &[("audit", RetentionHorizon::Forever)])
        .block_delete_passes(now())
        .unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].include_tags, None);
    assert_eq!(passes[0].exclude_tags, tags(&["audit"]));
}

#[test]
fn same_horizon_tags_share_a_pass() {
    let passes = policy(
        90,
        &[
            ("debug", RetentionHorizon::Days(3)),
            ("trace", RetentionHorizon::Days(3)),
        ],
    )
    .block_delete_passes(now())
    .unwrap();
    assert_eq!(passes.len(), 2);
    assert_eq!(passes[1].cutoff, ago(3));
    assert_eq!(passes[1].include_tags, Some(tags(&["debug", "trace"])));
    assert!(passes[1].exclude_tags.is_empty());
}

#[test]
fn longest_horizon_wins() {
    let passes = policy(
        90,
        &[
            ("a", RetentionHorizon::Days(3)),
            ("b", RetentionHorizon::Days(30)),
            ("c", RetentionHorizon::Forever),
        ],
    )
    .block_delete_passes(now())
    .unwrap();
    assert_eq!(passes.len(), 3);
    assert_eq!(passes[0].exclude_tags, tags(&["a", "b", "c"]));
    assert_eq!(passes[1].cutoff, ago(3));
    assert_eq!(passes[1].include_tags, Some(tags(&["a"])));
    assert_eq!(passes[1].exclude_tags, tags(&["b", "c"]));
    assert_eq!(passes[2].cutoff, ago(30));
    assert_eq!(passes[2].include_tags, Some(tags(&["b"])));
    assert_eq!(passes[2].exclude_tags, tags(&["c"]));
}

#[test]
fn overrides_longer_and_shorter_than_default_get_own_cutoff() {
    let passes = policy(
        10,
        &[
            ("short", RetentionHorizon::Days(2)),
            ("long", RetentionHorizon::Days(400)),
        ],
    )
    .block_delete_passes(now())
    .unwrap();
    let cutoffs: Vec<_> = passes.iter().map(|p| p.cutoff).collect();
    assert_eq!(cutoffs, vec![ago(10), ago(2), ago(400)]);
}

#[test]
fn default_cutoff_is_now_minus_default_days() {
    assert_eq!(policy(45, &[]).default_cutoff(now()).unwrap(), ago(45));
}
