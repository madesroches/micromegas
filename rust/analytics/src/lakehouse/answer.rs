use datafusion::arrow::{array::RecordBatch, datatypes::Schema};
use std::sync::Arc;

pub struct Answer {
    pub schema: Arc<Schema>,
    pub record_batches: Vec<RecordBatch>,
}

impl Answer {
    pub fn new(schema: Arc<Schema>, record_batches: Vec<RecordBatch>) -> Self {
        Self {
            schema,
            record_batches,
        }
    }
}
