///
/// to update the protos, run `cargo run --bin update-perfetto-protos --features=protogen`
///
use anyhow::{Context, Result};

fn main() -> Result<()> {
    std::env::set_var("OUT_DIR", "perfetto/src/");
    let proto_include_paths = vec![std::env::var("PERFETTO_SRC_PATH")
        .with_context(|| "reading environment variable PERFETTO_SRC_PATH")?];
    let protos = vec!["protos/perfetto/trace/trace.proto"];
    prost_build::compile_protos(&protos, &proto_include_paths)?;
    Ok(())
}
