fn main() {
    // `regen_protos` empties the library modules so the `update-perfetto-protos`
    // binary can build even when the committed `perfetto.protos.rs` does not
    // compile. It is intentionally driven by an env var rather than a Cargo
    // feature, so `--all-features` builds (e.g. `cargo doc`) keep the full API.
    println!("cargo::rustc-check-cfg=cfg(regen_protos)");
    println!("cargo::rerun-if-env-changed=MICROMEGAS_REGEN_PROTOS");
    if std::env::var_os("MICROMEGAS_REGEN_PROTOS").is_some() {
        println!("cargo::rustc-cfg=regen_protos");
    }
}
