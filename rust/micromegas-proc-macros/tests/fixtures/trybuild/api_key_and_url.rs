mod micromegas {
    pub use micromegas_telemetry_sink as telemetry_sink;
    pub mod tracing {
        pub use micromegas_tracing::levels;
        pub use micromegas_tracing::runtime;
    }
}

#[micromegas_proc_macros::micromegas_main(
    api_key = "test-api-key",
    telemetry_url = "http://localhost:9000"
)]
async fn main() {}
