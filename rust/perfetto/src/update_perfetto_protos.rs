use anyhow::Result;

fn main() -> Result<()> {
	std::env::set_var("OUT_DIR", "perfetto/src/");
    let proto_include_paths = vec!["F:/git/external/perfetto/"];
    let protos = vec!["protos/perfetto/trace/trace.proto"];
    prost_build::compile_protos(&protos, &proto_include_paths)?;
    Ok(())
}
