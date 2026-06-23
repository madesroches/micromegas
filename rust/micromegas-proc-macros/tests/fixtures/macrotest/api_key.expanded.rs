fn main() {
    let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
        .map(|v| v == "true")
        .unwrap_or(false);
    let _telemetry_guard = micromegas::telemetry_sink::TelemetryGuardBuilder::default()
        .with_process_property("version".to_string(), "0.0.0".to_string())
        .with_ctrlc_handling()
        .with_local_sink_max_level(micromegas::tracing::levels::LevelFilter::Debug)
        .with_telemetry_sink_url("http://localhost:9000".to_string())
        .with_request_decorator(
            std::boxed::Box::new(move || std::sync::Arc::new(
                micromegas::telemetry_sink::api_key_decorator::ApiKeyRequestDecorator::new(
                    "test-api-key".to_string(),
                ),
            )),
        )
        .build();
    let runtime = {
        use micromegas::tracing::runtime::TracingRuntimeExt;
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        builder.thread_name("micromegas-proc-macros-tests");
        if cpu_tracing_enabled {
            builder.with_tracing_callbacks();
        }
        builder.build().expect("Failed to build tokio runtime")
    };
    runtime.block_on(async move { {} })
}
