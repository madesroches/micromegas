use anyhow::{Context, Result};
use async_stream::try_stream;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::{
        self,
        array::{
            ListBuilder, RecordBatch, StringArray, StringBuilder, StructBuilder,
            TimestampNanosecondArray,
        },
        datatypes::{DataType, Field, Fields, TimestampNanosecondType},
    },
    catalog::MemTable,
    error::DataFusionError,
    logical_expr::ScalarUDF,
    physical_plan::stream::RecordBatchReceiverStreamBuilder,
    prelude::*,
    scalar::ScalarValue,
};
use futures::stream::BoxStream;
use futures::StreamExt;
use micromegas_analytics::{
    dfext::typed_column::{
        get_only_primitive_value, get_only_string_value, get_single_row_primitive_value_by_name,
        typed_column_by_name,
    },
    lakehouse::property_get_function::PropertyGet,
    time::TimeRange,
};
use std::{collections::HashMap, sync::Arc};

use super::{flightsql_client::Client, flightsql_client_factory::FlightSQLClientFactory};

pub fn budget_map_to_properties(
    span_name_to_budget: &HashMap<String, String>,
) -> Result<ScalarValue> {
    let prop_struct_fields = vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ];
    let prop_field = Arc::new(Field::new(
        "Property",
        DataType::Struct(Fields::from(prop_struct_fields.clone())),
        false,
    ));
    let mut props_builder =
        ListBuilder::new(StructBuilder::from_fields(prop_struct_fields, 10)).with_field(prop_field);

    for (k, v) in span_name_to_budget.iter() {
        let property_builder = props_builder.values();
        let key_builder = property_builder
            .field_builder::<StringBuilder>(0)
            .with_context(|| "getting key field builder")?;
        key_builder.append_value(k);
        let value_builder = property_builder
            .field_builder::<StringBuilder>(1)
            .with_context(|| "getting value field builder")?;
        value_builder.append_value(v);
        property_builder.append(true);
    }
    props_builder.append(true);
    let array = props_builder.finish();
    Ok(ScalarValue::List(Arc::new(array)))
}

pub fn get_record_batch_time_range(rb: &RecordBatch) -> Result<Option<TimeRange>> {
    if rb.num_rows() == 0 {
        return Ok(None);
    }
    let begin_column: &TimestampNanosecondArray = typed_column_by_name(rb, "begin")?;
    let end_column: &TimestampNanosecondArray = typed_column_by_name(rb, "end")?;
    let min_begin = DateTime::from_timestamp_nanos(
        arrow::compute::min(begin_column).with_context(|| "min(begin)")?,
    );
    let max_end = DateTime::from_timestamp_nanos(
        arrow::compute::max(end_column).with_context(|| "max(end)")?,
    );
    Ok(Some(TimeRange::new(min_begin, max_end)))
}

pub async fn fetch_spans_batch(
    client: &mut Client,
    stream_id: &str,
    frames_rb: RecordBatch,
    span_to_budget: &HashMap<String, String>,
) -> Result<Vec<RecordBatch>> {
    let time_range = get_record_batch_time_range(&frames_rb)?;
    if time_range.is_none() {
        return Ok(vec![]);
    }
    let time_range = time_range.unwrap();
    let sql = format!(
        "SELECT name, begin, end, duration
         FROM view_instance('thread_spans', '{stream_id}')
         "
    );
    let spans_rbs = client.query(sql, Some(time_range)).await?;
    // add budget column locally
    let ctx = SessionContext::new();
    let table = MemTable::try_new(spans_rbs[0].schema(), vec![spans_rbs])?;
    ctx.register_table("spans", Arc::new(table))?;
    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));

    let spans = ctx
        .sql(
            "SELECT name, begin, end, duration, property_get($span_to_budget_map, name) as budget
             FROM spans
             WHERE property_get($span_to_budget_map, name) IS NOT NULL",
        )
        .await?
        .with_param_values(vec![(
            "span_to_budget_map",
            budget_map_to_properties(span_to_budget)?,
        )])?
        .collect()
        .await?;
    Ok(spans)
}

pub async fn extract_top_offenders(ctx: &SessionContext) -> Result<Vec<RecordBatch>> {
    let budgets_rbs = ctx
        .sql("SELECT DISTINCT budget FROM frame_stats ORDER BY budget")
        .await?
        .collect()
        .await?;
    let top_offenders_df = ctx
        .sql(
            "SELECT budget, duration_in_frame, begin_frame, end_frame, process_id
             FROM frame_stats
             WHERE budget = $budget
             ORDER BY duration_in_frame DESC
             LIMIT 100
             ",
        )
        .await?;
    let mut builder =
        RecordBatchReceiverStreamBuilder::new(top_offenders_df.schema().inner().clone(), 100);
    for budgets_rb in budgets_rbs {
        let budget_column: &StringArray = typed_column_by_name(&budgets_rb, "budget")?;
        for budget_row in 0..budgets_rb.num_rows() {
            let budget = budget_column.value(budget_row);
            let df = top_offenders_df
                .clone()
                .with_param_values(vec![("budget", ScalarValue::Utf8(Some(budget.to_owned())))])?;
            let sender = builder.tx();
            builder.spawn(async move {
                for rb in df.collect().await? {
                    sender.send(Ok(rb)).await.map_err(|e| {
                        DataFusionError::Execution(format!("sending record batch: {e:?}"))
                    })?;
                }
                Ok(())
            });
        }
    }
    let mut top_offenders_rbs = vec![];
    let mut top_stream = builder.build();
    while let Some(rb_res) = top_stream.next().await {
        top_offenders_rbs.push(rb_res?);
    }
    Ok(top_offenders_rbs)
}

pub async fn compute_frame_stats_for_batch(
    ctx: &SessionContext,
    frames_rb: RecordBatch,
    process_id: &str,
) -> Result<Vec<RecordBatch>> {
    let frame_stats_df = ctx
        .sql(
            "SELECT budget,
                    count(*) as count_in_frame,
                    sum(duration) as duration_in_frame,
                    to_timestamp_nanos($begin_frame) as begin_frame,
                    to_timestamp_nanos($end_frame) as end_frame,
                    arrow_cast($process_id, 'Utf8') as process_id
             FROM spans
             WHERE begin >= $begin_frame
             AND end <= $end_frame
             GROUP BY budget
             ",
        )
        .await
        .with_context(|| "frame_stats_df")?;

    let mut builder =
        RecordBatchReceiverStreamBuilder::new(frame_stats_df.schema().inner().clone(), 100);
    let utc: Arc<str> = Arc::from("+00:00");
    let begin_frame_column: &TimestampNanosecondArray =
        typed_column_by_name(&frames_rb, "begin")
            .map_err(|e| DataFusionError::Execution(format!("{e:?}")))?;
    let end_frame_column: &TimestampNanosecondArray = typed_column_by_name(&frames_rb, "end")
        .map_err(|e| DataFusionError::Execution(format!("{e:?}")))?;
    for iframe in 0..frames_rb.num_rows() {
        let begin_frame = begin_frame_column.value(iframe);
        let end_frame = end_frame_column.value(iframe);
        let df = frame_stats_df.clone().with_param_values(vec![
            (
                "begin_frame",
                ScalarValue::TimestampNanosecond(Some(begin_frame), Some(utc.clone())),
            ),
            (
                "end_frame",
                ScalarValue::TimestampNanosecond(Some(end_frame), Some(utc.clone())),
            ),
            ("process_id", ScalarValue::Utf8(Some(process_id.to_owned()))),
        ])?;
        let sender = builder.tx();
        builder.spawn(async move {
            for rb in df.collect().await? {
                sender.send(Ok(rb)).await.map_err(|e| {
                    DataFusionError::Execution(format!("sending record batch: {e:?}"))
                })?;
            }
            Ok(())
        });
    }

    let mut frame_stats_rbs = vec![];
    let mut stream = builder.build();
    while let Some(rb_res) = stream.next().await {
        frame_stats_rbs.push(rb_res?);
    }
    Ok(frame_stats_rbs)
}

pub async fn merge_top_offenders(top_offenders: Vec<RecordBatch>) -> Result<Vec<RecordBatch>> {
    if top_offenders.is_empty() {
        return Ok(top_offenders);
    }
    let ctx = SessionContext::new();
    let table = MemTable::try_new(top_offenders[0].schema(), vec![top_offenders])?;
    // it works because offenders have the same schema as frame_stats entries
    ctx.register_table("frame_stats", Arc::new(table))?;
    extract_top_offenders(&ctx).await
}

pub async fn process_frame_batch(
    ctx: &SessionContext,
    frames_rb: RecordBatch,
    process_id: &str,
) -> Result<(Vec<RecordBatch>, Vec<RecordBatch>)> {
    let frame_stats_rbs = compute_frame_stats_for_batch(ctx, frames_rb, process_id).await?;
    let ctx = SessionContext::new(); // new temp context to keep frame_stats from leaking out
    let table = MemTable::try_new(frame_stats_rbs[0].schema(), vec![frame_stats_rbs])?;
    ctx.register_table("frame_stats", Arc::new(table))?;
    let agg_rbs = ctx
        .sql(
            "SELECT budget,
                    count(*) as nb_frames,
                    sum(count_in_frame) as sum_counts,
                    sum(duration_in_frame) as sum_duration,
                    min(duration_in_frame) as min_duration,
                    max(duration_in_frame) as max_duration
             FROM frame_stats
             GROUP BY budget
             ",
        )
        .await?
        .collect()
        .await?;
    let top_offenders_rbs = extract_top_offenders(&ctx).await?;
    Ok((agg_rbs, top_offenders_rbs))
}

pub async fn get_process_start_time(
    client: &mut Client,
    process_id: &str,
) -> Result<DateTime<Utc>> {
    let sql = format!(
        "SELECT start_time
         FROM processes
         WHERE process_id = '{process_id}'"
    );
    let rbs = client.query(sql, None).await?;
    let start_time =
        DateTime::from_timestamp_nanos(get_only_primitive_value::<TimestampNanosecondType>(&rbs)?);
    Ok(start_time)
}

pub async fn get_main_thread_stream_id(
    client: &mut Client,
    process_id: &str,
    main_thread_name: &str,
    start_time: DateTime<Utc>,
) -> Result<String> {
    let query_range = Some(TimeRange::new(
        start_time - TimeDelta::minutes(5),
        start_time + TimeDelta::minutes(5),
    ));
    let sql = format!(
        "SELECT stream_id
	 FROM streams
	 WHERE process_id = '{process_id}'
	 AND property_get(properties, 'thread-name') = '{main_thread_name}'"
    );
    let rbs = client.query(sql, query_range).await?;
    get_only_string_value(&rbs)
}

pub async fn get_stream_time_range(client: &mut Client, stream_id: &str) -> Result<TimeRange> {
    let sql = format!(
        "SELECT min(begin_time) as min_begin_time, max(end_time) as max_end_time
         FROM blocks
         WHERE stream_id='{stream_id}'"
    );
    let rbs = client.query(sql, None).await?;
    let begin = DateTime::from_timestamp_nanos(get_single_row_primitive_value_by_name::<
        TimestampNanosecondType,
    >(&rbs, "min_begin_time")?);
    let end = DateTime::from_timestamp_nanos(get_single_row_primitive_value_by_name::<
        TimestampNanosecondType,
    >(&rbs, "max_end_time")?);
    Ok(TimeRange::new(begin, end))
}

pub async fn get_frames(
    client: &mut Client,
    stream_id: &str,
    time_range: TimeRange,
    top_level_span_name: &str,
) -> Result<Vec<RecordBatch>> {
    let sql = format!(
        "SELECT begin, end
         FROM view_instance('thread_spans', '{stream_id}')
         WHERE name = '{top_level_span_name}'
         ORDER BY begin"
    );
    client.query(sql, Some(time_range)).await
}

pub fn gen_frame_batches(
    frames_record_batches: Vec<RecordBatch>,
) -> BoxStream<'static, Result<RecordBatch>> {
    Box::pin(try_stream! {
        for b in frames_record_batches
        {
        if b.num_rows() == 0{
            continue;
        }

        let max_slice_size = 1024;
        let nb_slices = (b.num_rows() / max_slice_size) + 1;
        for islice in 0..nb_slices {
            let begin_index = islice * max_slice_size;
            if begin_index >= b.num_rows() {
            // can happen when num_rows == max_slice_size
            break;
            }
            let end_index = std::cmp::min((islice + 1) * max_slice_size, b.num_rows());
            let b = b.slice(begin_index, end_index - begin_index);
            yield b;
        }
        }
    })
}

pub async fn gen_span_batches(
    sender: tokio::sync::mpsc::Sender<(RecordBatch, Vec<RecordBatch>)>,
    client_factory: Arc<dyn FlightSQLClientFactory>,
    process_id: &str,
    main_thread_name: &str,
    top_level_span_name: &str,
    span_to_budget: &HashMap<String, String>,
) -> Result<()> {
    let mut client = client_factory.make_client().await?;
    let start_time = get_process_start_time(&mut client, process_id)
        .await
        .with_context(|| "Process not found")?;
    let main_thread_stream_id =
        get_main_thread_stream_id(&mut client, process_id, main_thread_name, start_time).await?;
    let main_thread_time_range = get_stream_time_range(&mut client, &main_thread_stream_id).await?;
    let frames_record_batches = get_frames(
        &mut client,
        &main_thread_stream_id,
        main_thread_time_range,
        top_level_span_name,
    )
    .await?;
    let mut frame_batch_stream = gen_frame_batches(frames_record_batches);
    while let Some(res) = frame_batch_stream.next().await {
        let frame_batch = res?;
        let spans_rbs = fetch_spans_batch(
            &mut client,
            &main_thread_stream_id,
            frame_batch.clone(),
            span_to_budget,
        )
        .await?;
        sender.send((frame_batch, spans_rbs)).await?;
    }
    Ok(())
}
