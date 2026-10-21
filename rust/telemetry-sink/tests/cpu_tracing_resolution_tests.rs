use micromegas_telemetry_sink::{ENABLE_CPU_TRACING_ENV_VAR, resolve_cpu_tracing_enabled};

#[test]
fn env_var_name_is_the_documented_one() {
    assert_eq!(ENABLE_CPU_TRACING_ENV_VAR, "MICROMEGAS_ENABLE_CPU_TRACING");
}

#[test]
fn explicit_true_wins_over_env_false() {
    assert!(resolve_cpu_tracing_enabled(Some(true), Some("false")));
}

#[test]
fn explicit_false_wins_over_env_true() {
    assert!(!resolve_cpu_tracing_enabled(Some(false), Some("true")));
}

#[test]
fn none_falls_back_to_env() {
    assert!(resolve_cpu_tracing_enabled(None, Some("true")));
}

#[test]
fn none_without_env_or_with_other_value_is_false() {
    assert!(!resolve_cpu_tracing_enabled(None, None));
    assert!(!resolve_cpu_tracing_enabled(None, Some("false")));
    assert!(!resolve_cpu_tracing_enabled(None, Some("1")));
}
