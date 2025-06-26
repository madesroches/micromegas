use anyhow::{bail, Context};
use async_stream::try_stream;
use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Sink;
use micromegas::chrono::DateTime;
use micromegas::datafusion::arrow::array::{
    ArrayRef, Int64Array, StringArray, TimestampNanosecondArray,
};
use micromegas::{
    client::flightsql_client_factory::FlightSQLClientFactory, datafusion::arrow, tracing::info,
};
use pgwire::api::results::QueryResponse;
use pgwire::api::Type;
use pgwire::{
    api::{
        query::SimpleQueryHandler,
        results::{DataRowEncoder, FieldFormat, FieldInfo, Response},
        ClientInfo, ClientPortalStore,
    },
    error::{PgWireError, PgWireResult},
    messages::PgWireBackendMessage,
};
use std::fmt::Debug;
use std::sync::Arc;

pub struct SimpleQueryH {
    flight_client_factory: Arc<dyn FlightSQLClientFactory>,
}

impl SimpleQueryH {
    pub fn new(flight_client_factory: Arc<dyn FlightSQLClientFactory>) -> Self {
        Self {
            flight_client_factory,
        }
    }
}

fn arrow_to_pg_type(arrow_type: &arrow::datatypes::DataType) -> anyhow::Result<Type> {
    match arrow_type {
        arrow::datatypes::DataType::Null => Ok(Type::UNKNOWN),
        arrow::datatypes::DataType::Boolean => Ok(Type::BOOL),
        arrow::datatypes::DataType::Int8 => Ok(Type::INT2),
        arrow::datatypes::DataType::Int16 => Ok(Type::INT2),
        arrow::datatypes::DataType::Int32 => Ok(Type::INT4),
        arrow::datatypes::DataType::Int64 => Ok(Type::INT8),
        arrow::datatypes::DataType::UInt8 => bail!("not yet implemented"),
        arrow::datatypes::DataType::UInt16 => bail!("not yet implemented"),
        arrow::datatypes::DataType::UInt32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::UInt64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float16 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Timestamp(_time_unit, _opt_time_zone) => Ok(Type::TIMESTAMP),
        arrow::datatypes::DataType::Date32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Date64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Time32(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Time64(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Duration(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Interval(_interval_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Binary => bail!("not yet implemented"),
        arrow::datatypes::DataType::FixedSizeBinary(_) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeBinary => bail!("not yet implemented"),
        arrow::datatypes::DataType::BinaryView => bail!("not yet implemented"),
        arrow::datatypes::DataType::Utf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::LargeUtf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::Utf8View => Ok(Type::TEXT),
        arrow::datatypes::DataType::List(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::ListView(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::FixedSizeList(_field, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeList(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeListView(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Struct(_fields) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Union(_union_fields, _union_mode) => {
            bail!("not yet implemented")
        }
        arrow::datatypes::DataType::Dictionary(_data_type, _data_type1) => {
            bail!("not yet implemented")
        }
        arrow::datatypes::DataType::Decimal128(_, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Decimal256(_, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Map(_field, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::RunEndEncoded(_field, _field1) => bail!("not yet implemented"),
    }
}

fn arrow_to_pg_schema(
    arrow_schema: &arrow::datatypes::Schema,
) -> anyhow::Result<Arc<Vec<FieldInfo>>> {
    let mut res = vec![];
    for f in arrow_schema.fields().iter() {
        res.push(FieldInfo::new(
            f.name().to_string(),
            None,
            None,
            arrow_to_pg_type(f.data_type())?,
            FieldFormat::Text,
        ));
    }
    Ok(Arc::new(res))
}

fn encode_value(
    encoder: &mut DataRowEncoder,
    value_index: usize,
    column: &ArrayRef,
) -> anyhow::Result<()> {
    match column.data_type() {
        arrow::datatypes::DataType::Null => encoder.encode_field(&Option::<bool>::None)?,
        arrow::datatypes::DataType::Boolean => bail!("not yet implemented"),
        arrow::datatypes::DataType::Int8 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Int16 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Int32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Int64 => {
            let column = column
                .as_any()
                .downcast_ref::<Int64Array>()
                .with_context(|| "casting to Int64Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::UInt8 => {}
        arrow::datatypes::DataType::UInt16 => bail!("not yet implemented"),
        arrow::datatypes::DataType::UInt32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::UInt64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float16 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Float64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Timestamp(_time_unit, _opt_time_zone) => {
            let column = column
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .with_context(|| "casting to TimestampNanosecondArray")?;
            encoder.encode_field(&DateTime::from_timestamp_nanos(column.value(value_index)))?;
        }
        arrow::datatypes::DataType::Date32 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Date64 => bail!("not yet implemented"),
        arrow::datatypes::DataType::Time32(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Time64(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Duration(_time_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Interval(_interval_unit) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Binary => bail!("not yet implemented"),
        arrow::datatypes::DataType::FixedSizeBinary(_) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeBinary => bail!("not yet implemented"),
        arrow::datatypes::DataType::BinaryView => bail!("not yet implemented"),
        arrow::datatypes::DataType::Utf8 | arrow::datatypes::DataType::LargeUtf8 => {
            let column = column
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting to StringArray")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Utf8View => bail!("not yet implemented"),
        arrow::datatypes::DataType::List(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::ListView(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::FixedSizeList(_field, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeList(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::LargeListView(_field) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Struct(_fields) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Union(_union_fields, _union_mode) => {
            bail!("not yet implemented")
        }
        arrow::datatypes::DataType::Dictionary(_data_type, _data_type1) => {
            bail!("not yet implemented")
        }
        arrow::datatypes::DataType::Decimal128(_, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Decimal256(_, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::Map(_field, _) => bail!("not yet implemented"),
        arrow::datatypes::DataType::RunEndEncoded(_field, _field1) => bail!("not yet implemented"),
    }
    Ok(())
}

#[async_trait]
impl SimpleQueryHandler for SimpleQueryH {
    /// Provide your query implementation using the incoming query string.
    async fn do_query<'a, C>(&self, _client: &mut C, sql: &str) -> PgWireResult<Vec<Response<'a>>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("sql={sql}");
        let mut flight_client = self
            .flight_client_factory
            .make_client()
            .await
            .map_err(|e| PgWireError::ApiError(e.into()))?;
        let mut record_batch_stream = flight_client
            .query_stream(sql.into(), None)
            .await
            .map_err(|e| PgWireError::ApiError(e.into()))?;

        let mut record_batch = record_batch_stream
            .next()
            .await
            .ok_or_else(|| PgWireError::ApiError("empty stream".into()))?
            .map_err(|e| PgWireError::ApiError(e.into()))?;

        let schema = arrow_to_pg_schema(record_batch.schema_ref())
            .map_err(|e| PgWireError::ApiError(e.into()))?;
        let schema_copy = schema.clone();

        // don't know why rustfmt does not behave here..
        let data_row_stream = Box::pin(try_stream! {
            loop {
        for row_index in 0..record_batch.num_rows() {
                    let mut encoder = DataRowEncoder::new(schema.clone());
                    for column in record_batch.columns() {
            encode_value(&mut encoder, row_index, column).map_err(|e| PgWireError::ApiError(e.into()))?;
                    }
            yield encoder.finish().map_err(|e| PgWireError::ApiError(e.into()))?;
        }
        if let Some(rb_res) = record_batch_stream.next().await{
            record_batch = rb_res.map_err(|e| PgWireError::ApiError(e.into()))?;
        }
        else{
            break;
        }
            }
        });

        Ok(vec![Response::Query(QueryResponse::new(
            schema_copy,
            data_row_stream,
        ))])
    }
}
