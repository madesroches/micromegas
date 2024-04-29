use datafusion::arrow::{
    array::{as_struct_array, ListBuilder, StructBuilder},
    record_batch::RecordBatch,
};

pub fn make_empty_record_batch() -> RecordBatch {
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields([], 0));
    let array = list_builder.finish();
    as_struct_array(array.values()).into()
}
