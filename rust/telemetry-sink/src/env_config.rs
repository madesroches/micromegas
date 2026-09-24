//! Shared helper for the handful of `TelemetryGuardBuilder` fields that can be
//! set explicitly in code, through an environment variable, or left at a
//! built-in default, in that order of precedence.
use std::ffi::OsString;
use std::str::FromStr;

/// Pure core, unit-testable without touching the process environment.
///
/// Like [`crate::PROCESS_PROPERTIES_ENV_VAR`], an unset or blank (after
/// trimming) variable means "not set" and resolves to `None`; unlike it, a
/// non-UTF-8 value is an error rather than a lossy conversion, since there is
/// no sensible non-UTF-8 level or byte count. A value that fails
/// `T::from_str` is also an error, naming the variable and the offending
/// value so a typo fails loudly instead of silently falling back to a
/// default.
fn parse_env_value<T: FromStr>(
    name: &str,
    raw: Option<OsString>,
    expected: &str,
) -> anyhow::Result<Option<T>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw
        .into_string()
        .map_err(|raw| anyhow::anyhow!("invalid {name} {raw:?}: not valid UTF-8"))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<T>()
        .map(Some)
        .map_err(|_err| anyhow::anyhow!("invalid {name} {trimmed:?}: expected {expected}"))
}

/// Reads and parses environment variable `name`, following
/// [`parse_env_value`]'s rules.
pub(crate) fn env_override<T: FromStr>(name: &str, expected: &str) -> anyhow::Result<Option<T>> {
    parse_env_value(name, std::env::var_os(name), expected)
}

/// Pure core of [`resolve`], unit-testable without touching the process
/// environment.
fn resolve_from<T: FromStr>(
    explicit: Option<T>,
    name: &str,
    raw: Option<OsString>,
    default: T,
    expected: &str,
) -> anyhow::Result<T> {
    if let Some(explicit) = explicit {
        // An explicit value is a deliberate pin: the env is never even parsed,
        // so a bad value there cannot break a caller who set the field in code.
        return Ok(explicit);
    }
    Ok(parse_env_value(name, raw, expected)?.unwrap_or(default))
}

/// Resolves a builder field with precedence `explicit > env(name) > default`.
pub(crate) fn resolve<T: FromStr>(
    explicit: Option<T>,
    name: &str,
    default: T,
    expected: &str,
) -> anyhow::Result<T> {
    resolve_from(explicit, name, std::env::var_os(name), default, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_LEVEL: &str = "one of off, fatal, error, warn, info, debug, trace";

    #[test]
    fn unset_is_none() {
        assert_eq!(
            parse_env_value::<usize>("VAR", None, "a non-negative integer").unwrap(),
            None
        );
    }

    #[test]
    fn blank_is_none() {
        assert_eq!(
            parse_env_value::<usize>("VAR", Some("".into()), "a non-negative integer").unwrap(),
            None
        );
        assert_eq!(
            parse_env_value::<usize>("VAR", Some("  ".into()), "a non-negative integer").unwrap(),
            None
        );
    }

    #[test]
    fn valid_level_is_parsed_and_trimmed() {
        use micromegas_tracing::levels::LevelFilter;
        assert_eq!(
            parse_env_value::<LevelFilter>("VAR", Some("INFO".into()), EXPECTED_LEVEL).unwrap(),
            Some(LevelFilter::Info)
        );
        assert_eq!(
            parse_env_value::<LevelFilter>("VAR", Some(" debug ".into()), EXPECTED_LEVEL).unwrap(),
            Some(LevelFilter::Debug)
        );
    }

    #[test]
    fn invalid_level_names_the_variable_and_value() {
        let err = parse_env_value::<micromegas_tracing::levels::LevelFilter>(
            "MICROMEGAS_LOCAL_SINK_MAX_LEVEL",
            Some("verbose".into()),
            EXPECTED_LEVEL,
        )
        .expect_err("verbose is not a valid level");
        let message = err.to_string();
        assert!(
            message.contains("MICROMEGAS_LOCAL_SINK_MAX_LEVEL"),
            "{message}"
        );
        assert!(message.contains("verbose"), "{message}");
    }

    #[test]
    #[cfg(unix)]
    fn non_utf8_value_is_an_error() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![0xff, 0xfe]);
        let err = parse_env_value::<usize>("VAR", Some(raw), "a non-negative integer")
            .expect_err("non-UTF-8 input must be rejected");
        assert!(err.to_string().contains("VAR"));
    }

    #[test]
    fn usize_parses_or_errors() {
        assert_eq!(
            parse_env_value::<usize>("VAR", Some("123".into()), "a non-negative integer").unwrap(),
            Some(123)
        );
        parse_env_value::<usize>("VAR", Some("12x".into()), "a non-negative integer")
            .expect_err("12x is not a valid usize");
    }

    #[test]
    fn resolve_explicit_wins_over_env_and_default() {
        assert_eq!(
            resolve_from(
                Some(7_usize),
                "VAR",
                Some("9".into()),
                1,
                "a non-negative integer"
            )
            .unwrap(),
            7
        );
    }

    #[test]
    fn resolve_env_wins_over_default() {
        assert_eq!(
            resolve_from(
                None,
                "VAR",
                Some("9".into()),
                1_usize,
                "a non-negative integer"
            )
            .unwrap(),
            9
        );
    }

    #[test]
    fn resolve_default_when_both_absent() {
        assert_eq!(
            resolve_from(None, "VAR", None, 1_usize, "a non-negative integer").unwrap(),
            1
        );
    }

    #[test]
    fn resolve_invalid_env_is_an_error_only_when_nothing_explicit_is_set() {
        // An explicit value means the env value is never parsed, so a bad env
        // value is not an error here.
        assert_eq!(
            resolve_from(
                Some(7_usize),
                "VAR",
                Some("not-a-number".into()),
                1,
                "a non-negative integer"
            )
            .unwrap(),
            7
        );
        resolve_from(
            None,
            "VAR",
            Some("not-a-number".into()),
            1_usize,
            "a non-negative integer",
        )
        .expect_err("an invalid env value must fail when nothing explicit is set");
    }
}
