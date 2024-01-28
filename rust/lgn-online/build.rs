fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("./tests/echo.proto")?;
    tonic_build::compile_protos("./tests/sum.proto")?;
    Ok(())
}
