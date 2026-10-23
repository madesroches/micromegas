//! Tests for `micromegas::settings`: per-kind validation, row parsing, resolution order, the
//! `pinned` source, registry defaults and the shadowed-env check. None needs a database; the
//! tests that read process env are `#[serial]` and restore it through `EnvGuard`.

use micromegas::settings::{
    DbOverrides, EnvFallbacks, SETTINGS, SelfServiceSettings, SettingsSource, find_spec, shadowed,
};
use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use serde_json::{Value, json};
use serial_test::serial;
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
    ]))
    .expect("parse");
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
                .map(|s| s.env_var)
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

    for (key, applied) in [
        ("public_view_sets", json!(env.isolation.public_view_sets)),
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
}
