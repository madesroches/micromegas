use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::images::ImageMsgQueueAny;
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;

#[test]
#[serial]
fn test_send_image_captured_in_memory() {
    let guard = init_in_memory_tracing();
    let image_data = vec![0xAB_u8, 0xCD, 0xEF, 0x01, 0x02];
    micromegas_tracing::dispatch::send_image("screenshot_001", "png", image_data.clone());

    let state = guard.sink.state.lock().expect("lock sink state");
    assert!(
        state.image_stream_desc.is_some(),
        "image stream should be initialized"
    );
    assert_eq!(
        state.image_blocks.len(),
        1,
        "exactly one image block should have been flushed"
    );

    let block = &state.image_blocks[0];
    assert_eq!(
        block.nb_objects(),
        1,
        "block should contain one image event"
    );

    let mut found = false;
    for event in block.events.iter() {
        let ImageMsgQueueAny::ImageEvent(evt) = event;
        assert_eq!(evt.name.0, "screenshot_001", "image name mismatch");
        assert_eq!(evt.format.0, "png", "image format mismatch");
        assert_eq!(evt.data.0, image_data, "image data mismatch");
        found = true;
    }
    assert!(found, "no ImageEvent found in block");
}
