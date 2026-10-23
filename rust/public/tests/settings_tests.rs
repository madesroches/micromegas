//! Tests for `micromegas::settings`: per-kind validation, row parsing, resolution order, the
//! `pinned` source, registry defaults and the shadowed-env check. None needs a database; the
//! tests that read process env are `#[serial]` and restore it through `EnvGuard`.

use micromegas::settings::{
    DbOverrides, EnvFallbacks, SETTINGS, SelfServiceSettings, SettingsSource, find_spec,
    parse_tag_horizons, shadowed,
};
use micromegas_analytics::delete::RetentionHorizon;
use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use serde_json::{Value, json};
use serial_test::serial;
use std::collections::BTreeMap;
use std::sync::Arc;

fn spec(key: &str) -> &'static micromegas::settings::SettingSpec {
    find_spec(key).expect("registered key")
}

fn rows(pairs: &[(&str, Value)]) -> Vec<(String, Value)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn env_with(views: &[&str], self_service: SelfServiceSettings) -> EnvFallbacks {
    EnvFallbacks {
        isolation: Arc::new(IsolationConfig {
            public_view_sets: views.iter().map(|s| s.to_string()).collect(),
        }),
        self_service,
        retention_days: 90,
    }
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

#[test]
fn bool_accepts_a_json_bool_and_rejects_a_string() {
    let s = spec("self_service_mint");
    assert!(s.validate(&json!(true)).is_ok());
    assert!(s.validate(&json!(false)).is_ok());
    assert!(s.validate(&json!("true")).is_err());
    assert!(s.validate(&json!(1)).is_err());
}

#[test]
fn count_accepts_zero_and_rejects_negative_fractional_and_overflow() {
    let s = spec("self_service_max_keys_per_caller");
    assert!(s.validate(&json!(0)).is_ok());
    assert!(s.validate(&json!(100)).is_ok());
    assert!(s.validate(&json!(i64::MAX)).is_ok());
    assert!(s.validate(&json!(-1)).is_err());
    assert!(s.validate(&json!(1.5)).is_err());
    assert!(
        s.validate(&json!(u64::try_from(i64::MAX).unwrap() + 1))
            .is_err()
    );
    assert!(s.validate(&json!("5")).is_err());
}

#[test]
fn name_list_accepts_names_and_rejects_malformed_entries() {
    let s = spec("public_view_sets");
    assert!(s.validate(&json!([])).is_ok());
    assert!(s.validate(&json!(["a", "b"])).is_ok());
    assert!(s.validate(&json!([""])).is_err());
    assert!(s.validate(&json!([" a"])).is_err());
    assert!(s.validate(&json!(["a "])).is_err());
    assert!(s.validate(&json!(["a", "a"])).is_err());
    assert!(s.validate(&json!(["a[b"])).is_err());
    assert!(s.validate(&json!(["a]"])).is_err());
    assert!(s.validate(&json!(["a\"b"])).is_err());
    assert!(s.validate(&json!(["a", 1])).is_err());
    assert!(s.validate(&json!("a,b")).is_err());
    assert!(s.validate(&json!({"a": 1})).is_err());
}

// ---------------------------------------------------------------------------
// Row parsing
// ---------------------------------------------------------------------------

#[test]
fn days_accepts_the_range_and_rejects_everything_else() {
    let s = spec("retention_days");
    for ok in [json!(1), json!(36500)] {
        s.validate(&ok).expect("valid");
    }
    for bad in [json!(0), json!(36501), json!("90"), json!(1.5), json!(-1)] {
        assert!(s.validate(&bad).is_err(), "{bad}");
    }
}

#[test]
fn tag_horizons_accepts_a_map_and_rejects_malformed_ones() {
    let s = spec("retention_by_stream_tag");
    for ok in [json!({}), json!({"audit": "forever", "debug": 3})] {
        s.validate(&ok).expect("valid");
    }
    for bad in [
        json!([]),
        json!({"": 3}),
        json!({" a": 3}),
        json!({"a": 0}),
        json!({"a": "never"}),
        json!({"a": null}),
        json!({"a": 36501}),
    ] {
        assert!(s.validate(&bad).is_err(), "{bad}");
    }
}

#[test]
fn an_invalid_retention_by_stream_tag_fails_the_whole_parse() {
    let result = DbOverrides::from_rows(rows(&[
        ("self_service_mint", json!(true)),
        ("retention_by_stream_tag", json!({"x": 0})),
    ]));
    assert!(result.is_err());
}

#[tokio::test]
async fn payload_retention_resolves_db_then_env() {
    let env = || EnvFallbacks {
        retention_days: 45,
        ..env_with(&[], SelfServiceSettings::default())
    };
    let from_env = SettingsSource::with_overrides(env(), DbOverrides::default());
    let r = from_env.payload_retention().await.expect("resolve");
    assert_eq!(r.default_days, 45);
    assert!(r.by_stream_tag.is_empty());

    let from_db = SettingsSource::with_overrides(
        env(),
        DbOverrides {
            retention_days: Some(7),
            retention_by_stream_tag: Some(
                parse_tag_horizons("k", &json!({"audit": "forever"})).expect("parse"),
            ),
            ..DbOverrides::default()
        },
    );
    let r = from_db.payload_retention().await.expect("resolve");
    assert_eq!(r.default_days, 7);
    assert_eq!(
        r.by_stream_tag,
        BTreeMap::from([("audit".to_string(), RetentionHorizon::Forever)])
    );
}

#[test]
fn unknown_keys_are_ignored() {
    let parsed = DbOverrides::from_rows(rows(&[
        ("from_a_newer_replica", json!({"anything": [1, 2]})),
        ("self_service_mint", json!(true)),
    ]))
    .expect("parse");
    assert_eq!(parsed.self_service_mint, Some(true));
    assert_eq!(parsed.stored_keys(), vec!["self_service_mint"]);
}

#[test]
fn an_invalid_known_key_fails_the_whole_parse() {
    let result = DbOverrides::from_rows(rows(&[
        ("self_service_mint", json!(true)),
        ("self_service_max_keys_per_caller", json!(-3)),
    ]));
    assert!(result.is_err());
}

#[test]
fn every_registered_key_parses_into_its_field() {
    let parsed = DbOverrides::from_rows(rows(&[
        ("public_view_sets", json!(["a", "b"])),
        ("self_service_mint", json!(true)),
        ("self_service_max_claims_per_caller", json!(1)),
        ("self_service_max_keys_per_caller", json!(2)),
        ("self_service_max_grants_per_caller", json!(3)),
        ("retention_days", json!(30)),
        (
            "retention_by_stream_tag",
            json!({"audit": "forever", "debug": 3}),
        ),
    ]))
    .expect("parse");
    assert_eq!(parsed.retention_days, Some(30));
    assert_eq!(
        parsed.retention_by_stream_tag,
        Some(BTreeMap::from([
            ("audit".to_string(), RetentionHorizon::Forever),
            ("debug".to_string(), RetentionHorizon::Days(3)),
        ]))
    );
    assert_eq!(
        parsed.isolation.expect("isolation").public_view_sets,
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(parsed.self_service_mint, Some(true));
    assert_eq!(parsed.max_claims_per_caller, Some(1));
    assert_eq!(parsed.max_keys_per_caller, Some(2));
    assert_eq!(parsed.max_grants_per_caller, Some(3));
}

// ---------------------------------------------------------------------------
// Resolution order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db_beats_env_beats_default_for_self_service() {
    let env = SelfServiceSettings {
        mint_enabled: true,
        max_claims_per_caller: 7,
        max_keys_per_caller: 8,
        max_grants_per_caller: 9,
    };

    let env_only = SettingsSource::with_overrides(env_with(&[], env), DbOverrides::default());
    assert_eq!(env_only.self_service().await.expect("resolve"), env);

    let partial = SettingsSource::with_overrides(
        env_with(&[], env),
        DbOverrides {
            self_service_mint: Some(false),
            max_keys_per_caller: Some(1),
            ..DbOverrides::default()
        },
    );
    let resolved = partial.self_service().await.expect("resolve");
    assert!(!resolved.mint_enabled);
    assert_eq!(resolved.max_claims_per_caller, 7);
    assert_eq!(resolved.max_keys_per_caller, 1);
    assert_eq!(resolved.max_grants_per_caller, 9);
}

#[tokio::test]
async fn isolation_config_db_beats_env() {
    let a = SettingsSource::with_overrides(
        env_with(&["from_env"], SelfServiceSettings::default()),
        DbOverrides::default(),
    );
    assert_eq!(
        a.isolation_config().await.expect("a").public_view_sets,
        vec!["from_env".to_string()]
    );

    let override_with = |name: &str| DbOverrides {
        isolation: Some(Arc::new(IsolationConfig {
            public_view_sets: vec![name.to_string()],
        })),
        ..DbOverrides::default()
    };
    let env = || env_with(&["from_env"], SelfServiceSettings::default());
    let a = SettingsSource::with_overrides(env(), override_with("a"));
    let b = SettingsSource::with_overrides(env(), override_with("b"));
    assert_eq!(
        a.isolation_config().await.expect("a").public_view_sets,
        vec!["a".to_string()]
    );
    assert_eq!(
        b.isolation_config().await.expect("b").public_view_sets,
        vec!["b".to_string()]
    );
    assert_eq!(
        a.env_isolation_config().public_view_sets,
        vec!["from_env".to_string()]
    );
}

#[tokio::test]
#[serial]
async fn pinned_ignores_env() {
    let _guard = EnvGuard::set(&[
        ("MICROMEGAS_PUBLIC_VIEW_SETS", "from_env"),
        ("MICROMEGAS_SELF_SERVICE_MINT", "true"),
    ]);
    let pinned = SettingsSource::pinned(Arc::new(IsolationConfig {
        public_view_sets: vec!["pinned".to_string()],
    }));
    assert_eq!(
        pinned
            .isolation_config()
            .await
            .expect("resolve")
            .public_view_sets,
        vec!["pinned".to_string()]
    );
    assert_eq!(
        pinned.self_service().await.expect("resolve"),
        SelfServiceSettings::default()
    );
}

// ---------------------------------------------------------------------------
// Env fallbacks and registry defaults
// ---------------------------------------------------------------------------

/// Restores the touched variables (to unset) on drop so a failing assertion cannot leak state.
struct EnvGuard(Vec<&'static str>);

impl EnvGuard {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let guard = Self(
            SETTINGS
                .iter()
                .filter_map(|s| s.env_var)
                .chain(vars.iter().map(|(k, _)| *k))
                .collect(),
        );
        // SAFETY: callers are `#[serial]`.
        unsafe {
            for name in &guard.0 {
                std::env::remove_var(name);
            }
            for (k, v) in vars {
                std::env::set_var(k, v);
            }
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: callers are `#[serial]`.
        unsafe {
            for name in &self.0 {
                std::env::remove_var(name);
            }
        }
    }
}

#[test]
#[serial]
fn env_fallbacks_default_to_the_registry_defaults() {
    let _guard = EnvGuard::set(&[]);
    let env = EnvFallbacks::from_env().expect("from_env");
    assert!(env.isolation.public_view_sets.is_empty());
    assert_eq!(env.self_service, SelfServiceSettings::default());

    assert_eq!(env.retention_days, 90);
    let tags_default: Value =
        serde_json::from_str(spec("retention_by_stream_tag").default).expect("default is JSON");
    spec("retention_by_stream_tag")
        .validate(&tags_default)
        .expect("default validates");
    assert!(
        parse_tag_horizons("retention_by_stream_tag", &tags_default)
            .expect("parse")
            .is_empty()
    );

    for (key, applied) in [
        ("public_view_sets", json!(env.isolation.public_view_sets)),
        ("retention_days", json!(env.retention_days)),
        ("self_service_mint", json!(env.self_service.mint_enabled)),
        (
            "self_service_max_claims_per_caller",
            json!(env.self_service.max_claims_per_caller),
        ),
        (
            "self_service_max_keys_per_caller",
            json!(env.self_service.max_keys_per_caller),
        ),
        (
            "self_service_max_grants_per_caller",
            json!(env.self_service.max_grants_per_caller),
        ),
    ] {
        let default: Value = serde_json::from_str(spec(key).default).expect("default is JSON");
        assert_eq!(default, applied, "{key}");
        spec(key).validate(&default).expect("default validates");
    }
}

#[test]
#[serial]
fn env_fallbacks_read_every_variable() {
    let _guard = EnvGuard::set(&[
        ("MICROMEGAS_PUBLIC_VIEW_SETS", "a,b"),
        ("MICROMEGAS_SELF_SERVICE_MINT", "true"),
        ("MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER", "1"),
        ("MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER", "2"),
        (
            "MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER",
            "not-a-number",
        ),
    ]);
    let env = EnvFallbacks::from_env().expect("from_env");
    assert_eq!(
        env.isolation.public_view_sets,
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(env.self_service.mint_enabled);
    assert_eq!(env.self_service.max_claims_per_caller, 1);
    assert_eq!(env.self_service.max_keys_per_caller, 2);
    // An unparseable count warns and falls back to the default.
    assert_eq!(env.self_service.max_grants_per_caller, 50);
}

#[test]
#[serial]
fn env_fallbacks_fail_fast_on_a_malformed_public_view_sets() {
    let _guard = EnvGuard::set(&[("MICROMEGAS_PUBLIC_VIEW_SETS", r#"["log_stats"]"#)]);
    let err = EnvFallbacks::from_env().expect_err("malformed value must be rejected");
    assert!(err.to_string().contains("MICROMEGAS_PUBLIC_VIEW_SETS"));
}

// ---------------------------------------------------------------------------
// Shadowed env
// ---------------------------------------------------------------------------

#[test]
fn shadowed_reports_only_keys_that_are_both_stored_and_set_in_env() {
    let set_in_env =
        |var: &str| var == "MICROMEGAS_SELF_SERVICE_MINT" || var == "MICROMEGAS_PUBLIC_VIEW_SETS";
    let found = shadowed(
        &["self_service_mint", "self_service_max_keys_per_caller"],
        set_in_env,
    );
    let keys: Vec<&str> = found.iter().map(|s| s.key).collect();
    assert_eq!(keys, vec!["self_service_mint"]);

    assert!(shadowed(&[], set_in_env).is_empty());

    // A spec with no env var is never reported, whatever the environment says.
    assert!(shadowed(&["retention_by_stream_tag"], |_| true).is_empty());
}
