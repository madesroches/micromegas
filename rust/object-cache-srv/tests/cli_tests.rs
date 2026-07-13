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

#[test]
fn validate_accepts_defaults() {
    let cli = Cli::parse_from(minimal_argv());
    assert!(cli.validate().is_ok());
}

#[test]
fn validate_rejects_zero_block_size() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.block_size = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_max_concurrent_fetches() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.max_concurrent_fetches = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_demand_reserved_at_or_above_max_concurrent() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.max_concurrent_fetches = 4;
    cli.demand_reserved_fetches = 4;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_memory_budget_mb() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.memory_budget_mb = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_memory_budget_below_stream_window_floor() {
    let mut cli = Cli::parse_from(minimal_argv());
    // window_mb = permits_for_bytes(2 * DEMAND_WINDOW_BLOCKS * block_size) with the
    // default block_size; 1 MiB is far below that floor.
    cli.memory_budget_mb = 1;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_prefetch_queue_capacity() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.prefetch_queue_capacity = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_prefetch_worker_concurrency() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.prefetch_worker_concurrency = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_flushers() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.flushers = 0;
    assert!(cli.validate().is_err());
}

#[test]
fn validate_rejects_zero_write_buffer_mb() {
    let mut cli = Cli::parse_from(minimal_argv());
    cli.write_buffer_mb = 0;
    assert!(cli.validate().is_err());
}
