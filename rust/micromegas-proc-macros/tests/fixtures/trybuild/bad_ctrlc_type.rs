mod micromegas {
    pub use micromegas_telemetry_sink as telemetry_sink;
    pub mod tracing {
        pub use micromegas_tracing::levels;
        pub use micromegas_tracing::runtime;
    }
}

#[micromegas_proc_macros::micromegas_main(ctrlc_handling = "not-a-bool")]
async fn main() {}
