use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::metadata::ProcessMetadata;
use micromegas_tracing::dispatch::make_process_info;
use std::collections::HashMap;
use std::sync::Arc;

// Helper function to convert ProcessInfo to ProcessMetadata for tests
pub fn make_process_metadata(
    process_id: uuid::Uuid,
    parent_process_id: Option<uuid::Uuid>,
    properties: HashMap<String, String>,
) -> ProcessMetadata {
    let process_info = make_process_info(process_id, parent_process_id, properties.clone());
    let properties_jsonb = serialize_properties_to_jsonb(&properties).unwrap();
    ProcessMetadata {
        process_id: process_info.process_id,
        exe: process_info.exe,
        username: process_info.username,
        realname: process_info.realname,
        computer: process_info.computer,
        distro: process_info.distro,
        cpu_brand: process_info.cpu_brand,
        tsc_frequency: process_info.tsc_frequency,
        start_time: process_info.start_time,
        start_ticks: process_info.start_ticks,
        parent_process_id: process_info.parent_process_id,
        properties: Arc::new(properties_jsonb),
    }
}
