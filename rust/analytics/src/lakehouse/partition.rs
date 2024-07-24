pub struct Partition {
    pub table_set_name: String,
    pub table_instance_id: String,
    pub begin_insert_time: chrono::DateTime<chrono::Utc>,
    pub end_insert_time: chrono::DateTime<chrono::Utc>,
    pub min_event_time: chrono::DateTime<chrono::Utc>,
    pub max_event_time: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
    pub file_path: String,
    pub file_size: i64,
    pub file_schema_hash: Vec<u8>,
    pub source_data_hash: Vec<u8>,
}
