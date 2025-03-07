#[allow(
    clippy::doc_lazy_continuation,
    clippy::len_without_is_empty,
    clippy::large_enum_variant
)]

#[cfg(not(feature = "protogen"))]
pub mod protos {
    include!("perfetto.protos.rs");
}
