use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::images::{ImageBlock, ImageEvent, ImageStream};
use micromegas_transit::value::Value;
use micromegas_transit::{DynBlob, DynString};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_image_block_round_trip() {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ImageStream::new(1024 * 1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    let image_data = vec![0x89_u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    stream.get_events_mut().push(ImageEvent {
        time: 42,
        name: DynString("heatmap".to_owned()),
        format: DynString("png".to_owned()),
        data: DynBlob(image_data.clone()),
    });

    let mut block = stream.replace_block(Arc::new(ImageBlock::new(
        1024 * 1024,
        process_id,
        stream_id,
        0,
    )));
    Arc::get_mut(&mut block)
        .expect("exclusive ref to image block")
        .close();

    let encoded = block
        .encode_bin(&process_info)
        .expect("encoding image block");
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).expect("decoding cbor block");

    let stream_info = make_stream_info(&stream);
    let stream_metadata =
        StreamMetadata::from_stream_info(&stream_info).expect("building stream metadata");

    let mut events_parsed = 0;
    parse_block(&stream_metadata, &received_block.payload, |val| {
        if let Value::Object(obj) = &val {
            assert_eq!(
                obj.type_name.as_str(),
                "ImageEvent",
                "unexpected object type"
            );
            assert_eq!(
                obj.get::<i64>("time").expect("reading time"),
                42,
                "time mismatch"
            );
            assert_eq!(
                &*obj.get::<Arc<String>>("name").expect("reading name"),
                "heatmap",
                "name mismatch"
            );
            assert_eq!(
                &*obj.get::<Arc<String>>("format").expect("reading format"),
                "png",
                "format mismatch"
            );
            let parsed_data = obj.get::<Arc<Vec<u8>>>("data").expect("reading data");
            assert_eq!(*parsed_data, image_data, "image data mismatch");
            events_parsed += 1;
        }
        Ok(true)
    })
    .expect("parse_block");

    assert_eq!(events_parsed, 1, "expected exactly one parsed ImageEvent");
}
