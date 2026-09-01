//! Exports the `malloc_conf` weak symbol jemalloc reads at startup, so every
//! binary declaring jemalloc as its global allocator runs with background
//! purging and a tightened dirty decay instead of jemalloc's stock
//! `background_thread:false,dirty_decay_ms:10000`. See
//! `mkdocs/docs/admin/memory-allocator.md`.

/// NUL-terminated: jemalloc reads this symbol as a `const char *`
/// (`jemalloc/src/conf.c`, `jemalloc/src/jemalloc.c`).
///
/// `muzzy_decay_ms:0` is already jemalloc 5.3's default -- pinned explicitly
/// since the default has moved across releases, and because it means muzzy
/// pages are `MADV_DONTNEED`'d immediately, the opposite of what a higher
/// value would do.
pub const CONF: &[u8] = b"background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:0\0";

/// Declares the `_rjem_malloc_conf` symbol jemalloc's `conf.c` resolves at
/// precedence 1 (weaker than `_RJEM_MALLOC_CONF`, stronger than the
/// compile-time `--with-malloc-conf` default), pointing it at [`CONF`].
///
/// Must be invoked from a **binary** crate, immediately below its
/// `#[global_allocator]` static: jemalloc's C code defines
/// `_rjem_malloc_conf` as a weak definition, so nothing forces the linker to
/// pull our override out of an rlib -- only a binary's own object files are
/// always linked in full. `$crate` (rather than a bare `CONF`) is required
/// because this macro expands inside each calling binary crate, not inside
/// `telemetry-sink`.
#[macro_export]
macro_rules! declare_jemalloc_conf {
    () => {
        #[cfg(not(target_os = "windows"))]
        #[used]
        #[unsafe(export_name = "_rjem_malloc_conf")]
        pub static MICROMEGAS_MALLOC_CONF: Option<&'static u8> =
            Some(&$crate::jemalloc_conf::CONF[0]);
    };
}
