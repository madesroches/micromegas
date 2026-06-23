#[test]
fn trybuild_compile_tests() {
    let t = trybuild::TestCases::new();
    t.pass("tests/fixtures/trybuild/default.rs");
    t.pass("tests/fixtures/trybuild/ctrlc_off.rs");
    t.pass("tests/fixtures/trybuild/local_sink_off.rs");
    t.pass("tests/fixtures/trybuild/local_sink_level_info.rs");
    t.pass("tests/fixtures/trybuild/install_log_capture_on.rs");
    t.pass("tests/fixtures/trybuild/system_metrics_off.rs");
    t.pass("tests/fixtures/trybuild/telemetry_url.rs");
    t.pass("tests/fixtures/trybuild/api_key_and_url.rs");
    t.compile_fail("tests/fixtures/trybuild/bad_ctrlc_type.rs");
    t.compile_fail("tests/fixtures/trybuild/bad_unknown_arg.rs");
}

#[test]
fn macrotest_api_key_expansion() {
    macrotest::expand("tests/fixtures/macrotest/api_key.rs");
}
