fn main() {
    let options = lgn_api_codegen::RustOptions::default();

    lgn_api_codegen::generate!(
        lgn_api_codegen::Language::Rust(options),
        "apis",
        ["components"]
    );
}
