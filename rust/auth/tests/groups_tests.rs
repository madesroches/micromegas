//! No-DB unit tests for `micromegas_auth::groups::GroupGraph` and `DbGroupsConfig`.

use micromegas_auth::groups::{ADMINS_GROUP, DbGroupsConfig, GroupGraph};
use serial_test::serial;
use uuid::Uuid;

fn graph(groups: &[&str], members: &[(&str, &str)]) -> GroupGraph {
    GroupGraph::from_rows(
        groups.iter().map(|g| g.to_string()),
        members.iter().map(|(g, m)| (g.to_string(), m.to_string())),
    )
    .expect("valid rows")
}

#[test]
fn closure_direct_user_member() {
    let g = graph(&["a"], &[("a", "user:alice@example.com")]);
    assert_eq!(
        g.closure(Some("alice@example.com"), None),
        vec!["a".to_string()]
    );
    assert_eq!(
        g.closure(Some("bob@example.com"), None),
        Vec::<String>::new()
    );
}

#[test]
fn closure_via_wildcard() {
    let g = graph(&["a"], &[("a", "*")]);
    assert_eq!(
        g.closure(Some("anyone@example.com"), None),
        vec!["a".to_string()]
    );
    assert_eq!(g.closure(None, None), vec!["a".to_string()]);
}

/// A three-level chain: `alice ∈ a`, `group:a ∈ b`, `group:b ∈ c`. Alice's closure resolves `{a,
/// b, c}`; a caller who is a member of `c` alone (not `a`) resolves only `{c}` -- the edge
/// direction is "member -> groups containing it", walked upward from the caller, not downward
/// from `c`.
#[test]
fn closure_three_level_chain_and_edge_direction() {
    let g = graph(
        &["a", "b", "c"],
        &[
            ("a", "user:alice@example.com"),
            ("b", "group:a"),
            ("c", "group:b"),
        ],
    );
    assert_eq!(
        g.closure(Some("alice@example.com"), None),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );

    // A direct member of `c` alone does not also resolve `a`/`b` -- the walk goes upward only.
    let g2 = graph(&["c"], &[("c", "user:carol@example.com")]);
    assert_eq!(
        g2.closure(Some("carol@example.com"), None),
        vec!["c".to_string()]
    );
}

/// A diamond: `alice ∈ a`, `group:a ∈ b`, `group:a ∈ c`, `group:b ∈ d`, `group:c ∈ d` -- `d` is
/// reached through two paths but appears once in the closure.
#[test]
fn closure_diamond_dedupes() {
    let g = graph(
        &["a", "b", "c", "d"],
        &[
            ("a", "user:alice@example.com"),
            ("b", "group:a"),
            ("c", "group:a"),
            ("d", "group:b"),
            ("d", "group:c"),
        ],
    );
    assert_eq!(
        g.closure(Some("alice@example.com"), None),
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string()
        ]
    );
}

/// A cycle `group:a ∈ b`, `group:b ∈ a` terminates rather than looping forever -- the visited set
/// is what tolerates it at read time.
#[test]
fn closure_cycle_terminates() {
    let g = graph(
        &["a", "b"],
        &[
            ("a", "user:alice@example.com"),
            ("b", "group:a"),
            ("a", "group:b"),
        ],
    );
    assert_eq!(
        g.closure(Some("alice@example.com"), None),
        vec!["a".to_string(), "b".to_string()]
    );
}

#[test]
fn has_wildcard_admin_true_when_admins_reachable_via_wildcard() {
    let direct = graph(&[ADMINS_GROUP], &[(ADMINS_GROUP, "*")]);
    assert!(direct.has_wildcard_admin());

    // Reached through nesting, not just a direct row.
    let nested = graph(
        &["everyone", ADMINS_GROUP],
        &[("everyone", "*"), (ADMINS_GROUP, "group:everyone")],
    );
    assert!(nested.has_wildcard_admin());
}

#[test]
fn has_wildcard_admin_false_when_admins_is_user_only() {
    let g = graph(&[ADMINS_GROUP], &[(ADMINS_GROUP, "user:admin@example.com")]);
    assert!(!g.has_wildcard_admin());
}

#[test]
fn nesting_would_cycle_rejects_self_nesting() {
    let g = graph(&["a"], &[]);
    assert!(g.nesting_would_cycle("a", "a"));
}

#[test]
fn nesting_would_cycle_rejects_a_two_step_loop() {
    // `group:a` is already a member of `b` -- nesting `group:b` into `a` would close the loop.
    let g = graph(&["a", "b"], &[("b", "group:a")]);
    assert!(g.nesting_would_cycle("a", "b"));
}

#[test]
fn nesting_would_cycle_allows_a_non_cyclic_nesting() {
    let g = graph(&["a", "b", "c"], &[("b", "group:a")]);
    assert!(!g.nesting_would_cycle("c", "b"));
}

#[test]
fn from_rows_rejects_a_bad_group_name() {
    let err = GroupGraph::from_rows(vec!["not a valid name".to_string()], vec![])
        .expect_err("expected a bad group name to be rejected");
    assert!(err.to_string().contains("not a valid name"));
}

#[test]
fn from_rows_rejects_a_bad_selector() {
    let err = GroupGraph::from_rows(
        vec!["a".to_string()],
        vec![("a".to_string(), "not-a-selector".to_string())],
    )
    .expect_err("expected a bad selector to be rejected");
    assert!(err.to_string().contains("not-a-selector"));
}

// ---------------------------------------------------------------------------
// DbGroupsConfig::from_env_with_prefix -- the flat, unprefixed `MICROMEGAS_AUTH_
// CACHE_TTL_SECONDS` knob `DbApiKeyConfig`/`DbAudienceGrantsConfig` also read, ignoring
// whatever `prefix` is passed (see `db_audience_grants_tests.rs`'s identical coverage).
// ---------------------------------------------------------------------------

const GROUPS_CONFIG_UNPREFIXED_VAR: &str = "MICROMEGAS_AUTH_CACHE_TTL_SECONDS";
/// A role prefix that must have no effect on this knob -- passed to every call below to pin that.
const GROUPS_CONFIG_SOME_PREFIX: &str = "MICROMEGAS_1549_GROUPS_TESTS";

struct GroupsConfigEnvGuard;

impl Drop for GroupsConfigEnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(GROUPS_CONFIG_UNPREFIXED_VAR);
        }
    }
}

#[test]
#[serial]
fn groups_config_from_env_defaults_to_60_when_unset() {
    let _guard = GroupsConfigEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(GROUPS_CONFIG_UNPREFIXED_VAR);
    }
    assert_eq!(
        DbGroupsConfig::from_env_with_prefix(GROUPS_CONFIG_SOME_PREFIX).cache_ttl_secs,
        60
    );
}

#[test]
#[serial]
fn groups_config_from_env_reads_the_flat_unprefixed_var_regardless_of_prefix() {
    let _guard = GroupsConfigEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(GROUPS_CONFIG_UNPREFIXED_VAR, "120");
    }
    assert_eq!(
        DbGroupsConfig::from_env_with_prefix(GROUPS_CONFIG_SOME_PREFIX).cache_ttl_secs,
        120
    );
    assert_eq!(DbGroupsConfig::from_env_with_prefix("").cache_ttl_secs, 120);
}

const KEY_K: &str = "0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11";
const KEY_OTHER: &str = "7c1e2f3a-1b2c-4d3e-8f4a-5b6c7d8e9f00";

fn key(id: &str) -> Option<Uuid> {
    Some(Uuid::parse_str(id).expect("uuid"))
}

#[test]
fn closure_seeds_from_the_key_and_walks_upward() {
    let member = format!("key:{KEY_K}");
    let g = graph(
        &["a", "b", "everyone"],
        &[("a", &member), ("b", "group:a"), ("everyone", "*")],
    );
    assert_eq!(
        g.closure(None, key(KEY_K)),
        vec!["a".to_string(), "b".to_string(), "everyone".to_string()]
    );
    assert_eq!(g.closure(None, None), vec!["everyone".to_string()]);
    assert_eq!(
        g.closure(None, key(KEY_OTHER)),
        vec!["everyone".to_string()]
    );
}

#[test]
fn key_walk_never_reaches_admins() {
    let member = format!("key:{KEY_K}");
    let g = graph(
        &[ADMINS_GROUP, "g", "above"],
        &[
            (ADMINS_GROUP, &member),
            (ADMINS_GROUP, "group:g"),
            ("g", &member),
            ("g", "user:h@example.com"),
            ("above", "group:admins"),
        ],
    );
    assert_eq!(g.closure(None, key(KEY_K)), vec!["g".to_string()]);
    // A human in `g` still reaches admins and what is above it.
    assert_eq!(
        g.closure(Some("h@example.com"), None),
        vec![
            ADMINS_GROUP.to_string(),
            "above".to_string(),
            "g".to_string()
        ]
    );
}

#[test]
fn wildcard_admin_still_applies_to_a_key() {
    let g = graph(&[ADMINS_GROUP], &[(ADMINS_GROUP, "*")]);
    assert_eq!(g.closure(None, key(KEY_K)), vec![ADMINS_GROUP.to_string()]);
}

#[test]
fn any_principal_reaches_ignores_key_paths() {
    let member = format!("key:{KEY_K}");
    let g = graph(
        &[ADMINS_GROUP, "g"],
        &[(ADMINS_GROUP, &member), ("g", &member)],
    );
    assert!(!g.any_principal_reaches(ADMINS_GROUP));
}
