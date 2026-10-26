use datafusion::arrow::array::{
    Array, ArrayRef, BinaryArray, DictionaryArray, Float64Array, Int64Array, LargeBinaryArray,
    RecordBatch, StringArray, StringViewArray, UInt64Array,
};
use datafusion::arrow::datatypes::{Field, Int32Type, Schema};
use datafusion::datasource::MemTable;
use datafusion::prelude::{SessionConfig, SessionContext};
use datasketches::hll::{HllSketch, HllType};
use std::sync::Arc;

fn make_ctx(partitions: usize) -> SessionContext {
    let ctx =
        SessionContext::new_with_config(SessionConfig::new().with_target_partitions(partitions));
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);
    ctx
}

fn register_table(ctx: &SessionContext, name: &str, columns: Vec<(&str, ArrayRef)>, parts: usize) {
    let fields: Vec<Field> = columns
        .iter()
        .map(|(n, a)| Field::new(*n, a.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let rows = columns[0].1.len();
    let chunk = rows.div_ceil(parts).max(1);
    let mut partitions = vec![];
    let mut start = 0;
    while start < rows {
        let len = chunk.min(rows - start);
        let arrays = columns.iter().map(|(_, a)| a.slice(start, len)).collect();
        partitions.push(vec![RecordBatch::try_new(schema.clone(), arrays).unwrap()]);
        start += len;
    }
    ctx.register_table(
        name,
        Arc::new(MemTable::try_new(schema, partitions).unwrap()),
    )
    .unwrap();
}

fn strings(range: std::ops::Range<usize>) -> ArrayRef {
    Arc::new(StringArray::from_iter_values(
        range.map(|i| format!("user-{i}")),
    ))
}

async fn query_u64(ctx: &SessionContext, sql: &str) -> Vec<Option<u64>> {
    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    let mut out = vec![];
    for b in batches {
        let arr = b.column(0).as_any().downcast_ref::<UInt64Array>().unwrap();
        out.extend(arr.iter());
    }
    out
}

async fn one_u64(ctx: &SessionContext, sql: &str) -> Option<u64> {
    let v = query_u64(ctx, sql).await;
    assert_eq!(v.len(), 1);
    v[0]
}

async fn sketch_bytes(ctx: &SessionContext, sql: &str) -> Option<Vec<u8>> {
    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    let arr = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert_eq!(arr.len(), 1);
    (!arr.is_null(0)).then(|| arr.value(0).to_vec())
}

#[tokio::test]
async fn accuracy_100k() {
    let ctx = make_ctx(4);
    register_table(&ctx, "t", vec![("v", strings(0..100_000))], 4);
    let n = one_u64(&ctx, "SELECT hll_cardinality(make_hll(v)) FROM t")
        .await
        .unwrap();
    assert!((n as f64 - 100_000.0).abs() / 100_000.0 < 0.03, "got {n}");
}

#[tokio::test]
async fn small_counts_are_exact() {
    let ctx = make_ctx(1);
    register_table(&ctx, "t", vec![("v", strings(0..10))], 1);
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(v)) FROM t").await,
        Some(10)
    );
}

#[tokio::test]
async fn duplicates_nulls_and_empties_ignored() {
    let ctx = make_ctx(2);
    let v: ArrayRef = Arc::new(StringArray::from(vec![
        Some("a"),
        Some("a"),
        None,
        Some(""),
        Some("b"),
        Some("b"),
    ]));
    register_table(&ctx, "t", vec![("v", v)], 2);
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(v)) FROM t").await,
        Some(2)
    );
}

#[tokio::test]
async fn empty_group_is_null() {
    let ctx = make_ctx(2);
    let v: ArrayRef = Arc::new(StringArray::from(vec![None::<&str>, None, Some("")]));
    register_table(&ctx, "t", vec![("v", v)], 2);
    assert!(
        sketch_bytes(&ctx, "SELECT make_hll(v) FROM t")
            .await
            .is_none()
    );
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(v)) FROM t").await,
        None
    );
    assert!(
        sketch_bytes(
            &ctx,
            "SELECT merge_hll(s) FROM (SELECT make_hll(v) AS s FROM t)"
        )
        .await
        .is_none()
    );
    assert_eq!(one_u64(&ctx, "SELECT hll_cardinality(NULL)").await, None);
}

#[tokio::test]
async fn merge_equals_union() {
    let ctx = make_ctx(2);
    register_table(
        &ctx,
        "a",
        vec![
            ("v", strings(0..60_000)),
            ("g", Arc::new(Int64Array::from(vec![1; 60_000]))),
        ],
        2,
    );
    register_table(
        &ctx,
        "b",
        vec![
            ("v", strings(40_000..100_000)),
            ("g", Arc::new(Int64Array::from(vec![1; 60_000]))),
        ],
        2,
    );
    register_table(&ctx, "u", vec![("v", strings(0..100_000))], 2);
    ctx.sql("CREATE TABLE sketches AS SELECT make_hll(v) AS s FROM a UNION ALL SELECT make_hll(v) AS s FROM b")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let merged = one_u64(&ctx, "SELECT hll_cardinality(merge_hll(s)) FROM sketches")
        .await
        .unwrap();
    let direct = one_u64(&ctx, "SELECT hll_cardinality(make_hll(v)) FROM u")
        .await
        .unwrap();
    assert_eq!(merged, direct);
}

#[tokio::test]
async fn type_variants_agree() {
    let ctx = make_ctx(2);
    let values = ["a", "b", "c", "a", "d", "b"];
    let utf8: ArrayRef = Arc::new(StringArray::from(values.to_vec()));
    let view: ArrayRef = Arc::new(StringViewArray::from(values.to_vec()));
    let dict: ArrayRef = Arc::new(DictionaryArray::<Int32Type>::from_iter(
        values.iter().copied(),
    ));
    let bin: ArrayRef = Arc::new(BinaryArray::from_iter_values(
        values.iter().map(|s| s.as_bytes()),
    ));
    let large_bin: ArrayRef = Arc::new(LargeBinaryArray::from_iter_values(
        values.iter().map(|s| s.as_bytes()),
    ));
    register_table(
        &ctx,
        "t",
        vec![
            ("u", utf8),
            ("w", view),
            ("d", dict),
            ("b", bin),
            ("lb", large_bin),
        ],
        1,
    );
    let u = sketch_bytes(&ctx, "SELECT make_hll(u) FROM t")
        .await
        .unwrap();
    for col in ["w", "d", "b", "lb"] {
        let other = sketch_bytes(&ctx, &format!("SELECT make_hll({col}) FROM t"))
            .await
            .unwrap();
        assert_eq!(u, other, "column {col}");
    }
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(u)) FROM t").await,
        Some(4)
    );
}

#[tokio::test]
async fn numeric_types() {
    let ctx = make_ctx(1);
    let ints: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3, 3, 1]));
    let floats: ArrayRef = Arc::new(Float64Array::from(vec![1.5, 2.5, 2.5, -0.0, 0.0]));
    register_table(&ctx, "t", vec![("i", ints), ("f", floats)], 1);
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(i)) FROM t").await,
        Some(3)
    );
    // canonical float hashing folds -0.0 into 0.0
    assert_eq!(
        one_u64(&ctx, "SELECT hll_cardinality(make_hll(f)) FROM t").await,
        Some(3)
    );
    assert_eq!(
        one_u64(
            &ctx,
            "SELECT hll_cardinality(make_hll(CAST(i AS INT))) FROM t"
        )
        .await,
        Some(3)
    );
}

#[tokio::test]
async fn unsupported_type_is_plan_error() {
    let ctx = make_ctx(1);
    register_table(&ctx, "t", vec![("b", Arc::new(arrow_bool()))], 1);
    assert!(ctx.sql("SELECT make_hll(b) FROM t").await.is_err());
}

fn arrow_bool() -> datafusion::arrow::array::BooleanArray {
    datafusion::arrow::array::BooleanArray::from(vec![true, false])
}

#[tokio::test]
async fn mixed_lg_k_merge() {
    let mut low = HllSketch::new(12, HllType::Hll8).unwrap();
    for i in 0..50_000 {
        low.update(format!("low-{i}"));
    }
    let mut high = HllSketch::new(14, HllType::Hll4).unwrap();
    for i in 0..50_000 {
        high.update(format!("high-{i}"));
    }
    let sketches: ArrayRef = Arc::new(BinaryArray::from_iter_values([
        low.serialize(),
        high.serialize(),
    ]));
    let ctx = make_ctx(1);
    register_table(&ctx, "t", vec![("s", sketches)], 1);
    let n = one_u64(&ctx, "SELECT hll_cardinality(merge_hll(s)) FROM t")
        .await
        .unwrap();
    assert!((n as f64 - 100_000.0).abs() / 100_000.0 < 0.05, "got {n}");
}

#[tokio::test]
async fn invalid_bytes_error() {
    let ctx = make_ctx(1);
    let bad: ArrayRef = Arc::new(BinaryArray::from_iter_values([b"not a sketch".as_slice()]));
    register_table(&ctx, "t", vec![("s", bad)], 1);
    for sql in [
        "SELECT merge_hll(s) FROM t",
        "SELECT hll_cardinality(s) FROM t",
    ] {
        let res = ctx.sql(sql).await.unwrap().collect().await;
        let err = res.expect_err(sql).to_string();
        assert!(err.contains("invalid HyperLogLog sketch"), "{err}");
    }
}

#[tokio::test]
async fn materialized_view_shape() {
    let ctx = make_ctx(4);
    let n = 30_000usize;
    let day: ArrayRef = Arc::new(Int64Array::from_iter_values((0..n).map(|i| (i % 3) as i64)));
    // each day sees users 0..10_000 (overlap across days), so the month has 10_000 distinct
    let users: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..n).map(|i| format!("user-{}", i / 3)),
    ));
    register_table(&ctx, "events", vec![("day", day), ("user", users)], 4);
    ctx.sql("CREATE TABLE daily AS SELECT day, make_hll(user) AS s FROM events GROUP BY day")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let per_day = query_u64(&ctx, "SELECT hll_cardinality(s) FROM daily").await;
    assert_eq!(per_day.len(), 3);
    for d in per_day {
        let d = d.unwrap() as f64;
        assert!((d - 10_000.0).abs() / 10_000.0 < 0.03, "got {d}");
    }
    let mau = one_u64(&ctx, "SELECT hll_cardinality(merge_hll(s)) FROM daily")
        .await
        .unwrap() as f64;
    assert!((mau - 10_000.0).abs() / 10_000.0 < 0.03, "got {mau}");
}
