//! Regression test for the producer half of the segment-boundary overlap fix
//! (see `tasks/completed/thread_spans_segment_boundary_overlap_plan.md`, Part A):
//! consecutive blocks of a flushed stream must touch exactly, i.e.
//! `block[k].end == block[k+1].begin`, because the flush paths now derive
//! both stamps from a single `DualTime`.

use micromegas_tracing::dispatch::{flush_thread_buffer, init_thread_stream};
use micromegas_tracing::prelude::*;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use serial_test::serial;

#[test]
#[serial]
fn thread_block_boundary_stamps_touch_exactly() {
    let guard = init_in_memory_tracing();
    init_thread_stream();

    // emit -> flush -> emit -> flush: flushing twice with no second emit in
    // between yields only one block (the flush paths early-return on
    // is_empty()), so a second emit is mandatory to get a second block.
    {
        span_scope!("boundary_stamp_first");
    }
    flush_thread_buffer();

    {
        span_scope!("boundary_stamp_second");
    }
    flush_thread_buffer();

    let state = guard.sink.state.lock().expect("lock sink state");
    assert_eq!(
        state.thread_blocks.len(),
        2,
        "expected exactly two flushed thread blocks"
    );

    let first_end = state.thread_blocks[0]
        .end
        .expect("first block should be closed");
    let second_begin = state.thread_blocks[1].begin;

    assert_eq!(
        first_end, second_begin,
        "consecutive blocks must share the exact same boundary timestamp"
    );
}
