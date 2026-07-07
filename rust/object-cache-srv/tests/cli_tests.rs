//! Coverage for the two write-tuning CLI/env knobs added for the foyer 0.22
//! upgrade (`--flushers` / `--write-buffer-mb`): their defaults, and that
//! `validate_write_tuning` (the fatal-`anyhow!` startup guard `main` calls)
//! rejects zero for either.

use clap::Parser;
use micromegas_object_cache_srv::cli::{Cli, validate_write_tuning};

/// The minimal argv `Cli::parse_from` needs to succeed: `--origin-uri` and
/// `--disk-path` have no default and are otherwise required.
fn minimal_argv() -> Vec<&'static str> {
    vec![
        "micromegas-object-cache-srv",
        "--origin-uri",
        "s3://bucket",
        "--disk-path",
        "/tmp/does-not-need-to-exist",
    ]
}

#[test]
fn write_tuning_defaults() {
    let cli = Cli::parse_from(minimal_argv());
    assert_eq!(cli.flushers, 2);
    assert_eq!(cli.write_buffer_mb, 128);
}

#[test]
fn validate_write_tuning_rejects_zero_flushers() {
    assert!(validate_write_tuning(0, 128).is_err());
}

#[test]
fn validate_write_tuning_rejects_zero_write_buffer_mb() {
    assert!(validate_write_tuning(2, 0).is_err());
}

#[test]
fn validate_write_tuning_accepts_defaults() {
    assert!(validate_write_tuning(2, 128).is_ok());
}
