//! Static and runtime span names behind one interface, so that guards and futures are written once.
use crate::dispatch::{
    on_begin_async_named_scope, on_begin_async_named_scope_dyn, on_begin_named_scope,
    on_begin_named_scope_dyn, on_end_async_named_scope, on_end_async_named_scope_dyn,
    on_end_named_scope, on_end_named_scope_dyn,
};
use crate::spans::SpanLocation;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::StaticSpanName {}
    impl<N: AsRef<str>> Sealed for super::DynSpanName<N> {}
}

pub trait SpanName: sealed::Sealed {
    fn begin_thread(&self, loc: &'static SpanLocation);
    fn end_thread(&self, loc: &'static SpanLocation);
    fn begin_async(&self, loc: &'static SpanLocation, parent: u64, depth: u32) -> u64;
    fn end_async(&self, span_id: u64, parent: u64, loc: &'static SpanLocation, depth: u32);
}

/// A `&'static str` name. It is a lifetime-free wrapper because a trait implemented only for
/// `&'static str` makes rustc reject `Send` futures that await an instrumented future
/// ("implementation is not general enough").
#[derive(Clone, Copy)]
pub struct StaticSpanName(pub &'static str);

impl SpanName for StaticSpanName {
    #[inline(always)]
    fn begin_thread(&self, loc: &'static SpanLocation) {
        on_begin_named_scope(loc, self.0);
    }

    #[inline(always)]
    fn end_thread(&self, loc: &'static SpanLocation) {
        on_end_named_scope(loc, self.0);
    }

    #[inline(always)]
    fn begin_async(&self, loc: &'static SpanLocation, parent: u64, depth: u32) -> u64 {
        on_begin_async_named_scope(loc, self.0, parent, depth)
    }

    #[inline(always)]
    fn end_async(&self, span_id: u64, parent: u64, loc: &'static SpanLocation, depth: u32) {
        on_end_async_named_scope(span_id, parent, loc, self.0, depth);
    }
}

/// A span name built at runtime. It is copied into the block that records each event.
pub struct DynSpanName<N>(pub N);

impl<N: AsRef<str>> SpanName for DynSpanName<N> {
    #[inline(always)]
    fn begin_thread(&self, loc: &'static SpanLocation) {
        on_begin_named_scope_dyn(loc, self.0.as_ref());
    }

    #[inline(always)]
    fn end_thread(&self, loc: &'static SpanLocation) {
        on_end_named_scope_dyn(loc, self.0.as_ref());
    }

    #[inline(always)]
    fn begin_async(&self, loc: &'static SpanLocation, parent: u64, depth: u32) -> u64 {
        on_begin_async_named_scope_dyn(loc, self.0.as_ref(), parent, depth)
    }

    #[inline(always)]
    fn end_async(&self, span_id: u64, parent: u64, loc: &'static SpanLocation, depth: u32) {
        on_end_async_named_scope_dyn(span_id, parent, loc, self.0.as_ref(), depth);
    }
}
