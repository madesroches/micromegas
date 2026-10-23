use datafusion::arrow::array::{Array, Int32Array, StringArray};
use micromegas_analytics::lakehouse::images_view::ImagesViewMaker;
use micromegas_analytics::lakehouse::list_view_set_definitions_table_function::{
    DdlRow, build_batch, merge_rows,
};
use micromegas_analytics::lakehouse::log_view::{LogView, LogViewMaker};
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use std::sync::Arc;

fn factory() -> ViewFactory {
    let log = LogView::new("global").expect("global log view");
    let mut f = ViewFactory::new(vec![Arc::new(log)]);
    f.add_view_set("log_entries".into(), Arc::new(LogViewMaker {}));
    f.add_view_set("images".into(), Arc::new(ImagesViewMaker {}));
    f
}

fn ddl(name: &str) -> DdlRow {
    DdlRow {
        view_set_name: name.into(),
        definition_sql: "SELECT 1".into(),
        update_group: 7,
        updated_at_nanos: 42,
        updated_by: Some("admin".into()),
    }
}

#[test]
fn builtin_rows_have_null_definition_and_group_from_view() {
    let rows = merge_rows(vec![], &factory());
    let log = rows
        .iter()
        .find(|r| r.view_set_name == "log_entries")
        .unwrap();
    assert_eq!(log.source, "builtin");
    assert_eq!(log.update_group, Some(2000));
    assert!(log.definition_sql.is_none());
    assert!(log.updated_at_nanos.is_none());
    assert!(log.updated_by.is_none());
}

#[test]
fn maker_only_set_has_null_update_group() {
    let rows = merge_rows(vec![], &factory());
    let images = rows.iter().find(|r| r.view_set_name == "images").unwrap();
    assert_eq!(images.source, "builtin");
    assert!(images.update_group.is_none());
}

#[test]
fn postgres_row_wins_on_name_collision_and_output_is_sorted() {
    let rows = merge_rows(vec![ddl("zeta"), ddl("log_entries")], &factory());
    let names: Vec<&str> = rows.iter().map(|r| r.view_set_name.as_str()).collect();
    assert_eq!(names, vec!["images", "log_entries", "zeta"]);
    let log = &rows[1];
    assert_eq!(log.source, "ddl");
    assert_eq!(log.update_group, Some(7));
    assert_eq!(log.definition_sql.as_deref(), Some("SELECT 1"));
}

#[test]
fn batch_carries_nulls_and_source_column() {
    let rows = merge_rows(vec![ddl("zeta")], &factory());
    let rb = build_batch(&rows, None).unwrap();
    assert_eq!(rb.num_columns(), 6);
    assert_eq!(rb.schema().field(5).name(), "source");
    let sql = rb.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    let group = rb.column(2).as_any().downcast_ref::<Int32Array>().unwrap();
    let source = rb.column(5).as_any().downcast_ref::<StringArray>().unwrap();
    // images, log_entries, zeta
    assert!(sql.is_null(0) && sql.is_null(1) && !sql.is_null(2));
    assert!(group.is_null(0));
    assert_eq!(group.value(1), 2000);
    assert_eq!(source.value(0), "builtin");
    assert_eq!(source.value(2), "ddl");
    assert_eq!(build_batch(&rows, Some(2)).unwrap().num_rows(), 2);
}
