fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::var("OUT_DIR")?;
    let protos = &[
        "./protos/analytics.proto",
        "./protos/block.proto",
        "./protos/cache.proto",
        "./protos/callgraph.proto",
        "./protos/calltree.proto",
        "./protos/health.proto",
        "./protos/ingestion.proto",
        "./protos/metric.proto",
        "./protos/process.proto",
        "./protos/span.proto",
        "./protos/stream.proto",
    ];
    tonic_build::configure()
        .out_dir(out_dir)
        .compile(protos, &["protos"])?;

    for proto in protos {
        println!("cargo:rerun-if-changed={}", proto);
    }
    Ok(())
}
