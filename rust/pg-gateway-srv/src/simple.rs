use anyhow::{bail, Context};
use async_stream::try_stream;
use async_trait::async_trait;
use bytes::BufMut;
use futures::stream::StreamExt;
use futures::Sink;
use micromegas::chrono::DateTime;
use micromegas::datafusion::arrow::array::{
    ArrayRef, Float16Array, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, Int8Array, StringArray, TimestampNanosecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array
};
use micromegas::datafusion::arrow::json::writer::make_encoder;
use micromegas::datafusion::arrow::json::EncoderOptions;
use micromegas::{
    client::flightsql_client_factory::FlightSQLClientFactory, datafusion::arrow, tracing::info,
};
use pgwire::api::results::QueryResponse;
use pgwire::api::Type;
use pgwire::types::ToSqlText;
use pgwire::{
    api::{
        query::SimpleQueryHandler,
        results::{DataRowEncoder, FieldFormat, FieldInfo, Response},
        ClientInfo, ClientPortalStore,
    },
    error::{PgWireError, PgWireResult},
    messages::PgWireBackendMessage,
};
use postgres_types::ToSql;
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
        arrow::datatypes::DataType::UInt8 => Ok(Type::INT2),
        arrow::datatypes::DataType::UInt16 => Ok(Type::INT2),
        arrow::datatypes::DataType::UInt32 => Ok(Type::INT4),
        arrow::datatypes::DataType::UInt64 => Ok(Type::INT8),
        arrow::datatypes::DataType::Float16 => Ok(Type::FLOAT4),
        arrow::datatypes::DataType::Float32 => Ok(Type::FLOAT4),
        arrow::datatypes::DataType::Float64 => Ok(Type::FLOAT8),
        arrow::datatypes::DataType::Timestamp(_time_unit, _opt_time_zone) => Ok(Type::TIMESTAMP),
        arrow::datatypes::DataType::Date32 => bail!("DataType::Date32 not yet implemented"),
        arrow::datatypes::DataType::Date64 => bail!("DataType::Date64 not yet implemented"),
        arrow::datatypes::DataType::Time32(_time_unit) => {
            bail!("DataType::Time32 not yet implemented")
        }
        arrow::datatypes::DataType::Time64(_time_unit) => {
            bail!("DataType::Time64 not yet implemented")
        }
        arrow::datatypes::DataType::Duration(_time_unit) => {
            bail!("DataType::Duration not yet implemented")
        }
        arrow::datatypes::DataType::Interval(_interval_unit) => {
            bail!("DataType::Interval not yet implemented")
        }
        arrow::datatypes::DataType::Binary => bail!("DataType::Binary not yet implemented"),
        arrow::datatypes::DataType::FixedSizeBinary(_) => {
            bail!("DataType::FixedSizeBinary not yet implemented")
        }
        arrow::datatypes::DataType::LargeBinary => {
            bail!("DataType::LargeBinary not yet implemented")
        }
        arrow::datatypes::DataType::BinaryView => bail!("DataType::BinaryView not yet implemented"),
        arrow::datatypes::DataType::Utf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::LargeUtf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::Utf8View => Ok(Type::TEXT),
        arrow::datatypes::DataType::List(_field) => Ok(Type::JSON_ARRAY),
        arrow::datatypes::DataType::ListView(_field) => {
            bail!("DataType::ListView not yet implemented")
        }
        arrow::datatypes::DataType::FixedSizeList(_field, _) => {
            bail!("DataType::FixedSizeList not yet implemented")
        }
        arrow::datatypes::DataType::LargeList(_field) => {
            bail!("DataType::LargeList not yet implemented")
        }
        arrow::datatypes::DataType::LargeListView(_field) => {
            bail!("DataType::LargeListView not yet implemented")
        }
        arrow::datatypes::DataType::Struct(_fields) => {
            bail!("DataType::Struct not yet implemented")
        }
        arrow::datatypes::DataType::Union(_union_fields, _union_mode) => {
            bail!("DataType::Union not yet implemented")
        }
        arrow::datatypes::DataType::Dictionary(_data_type, _data_type1) => {
            bail!("DataType::Dictionary not yet implemented")
        }
        arrow::datatypes::DataType::Decimal128(_, _) => {
            bail!("DataType::Decimal128 not yet implemented")
        }
        arrow::datatypes::DataType::Decimal256(_, _) => {
            bail!("DataType::Decimal256 not yet implemented")
        }
        arrow::datatypes::DataType::Map(_field, _) => bail!("DataType::Map not yet implemented"),
        arrow::datatypes::DataType::RunEndEncoded(_field, _field1) => {
            bail!("DataType::RunEndEncoded not yet implemented")
        }
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

#[derive(Debug)]
struct ValueEncodedAsText {
    pub inner: String,
}

impl ToSql for ValueEncodedAsText {
    fn to_sql(
        &self,
        _ty: &Type,
        _out: &mut bytes::BytesMut,
    ) -> Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>>
    where
        Self: Sized,
    {
        Err("ValueEncodedAsText::to_sql not implemented".into())
    }

    fn accepts(_ty: &Type) -> bool
    where
        Self: Sized,
    {
        false
    }

    fn to_sql_checked(
        &self,
        _ty: &Type,
        _out: &mut bytes::BytesMut,
    ) -> Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        Err("ValueEncodedAsText::to_sql_checked not implemented".into())
    }
}

impl ToSqlText for ValueEncodedAsText {
    fn to_sql_text(
        &self,
        _ty: &Type,
        w: &mut micromegas::prost::bytes::BytesMut,
    ) -> Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>>
    where
        Self: Sized,
    {
        w.put_slice(self.inner.as_bytes());
        Ok(postgres_types::IsNull::No)
    }
}

fn encode_value(
    encoder: &mut DataRowEncoder,
    value_index: usize,
    column: &ArrayRef,
) -> anyhow::Result<()> {
    match column.data_type() {
        arrow::datatypes::DataType::Null => encoder.encode_field(&Option::<bool>::None)?,
        arrow::datatypes::DataType::Boolean => bail!("DataType::Boolean not yet implemented"),
        arrow::datatypes::DataType::Int8 => {
            let column = column
                .as_any()
                .downcast_ref::<Int8Array>()
                .with_context(|| "casting to Int8Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Int16 => {
            let column = column
                .as_any()
                .downcast_ref::<Int16Array>()
                .with_context(|| "casting to Int16Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Int32 => {
            let column = column
                .as_any()
                .downcast_ref::<Int32Array>()
                .with_context(|| "casting to Int32Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Int64 => {
            let column = column
                .as_any()
                .downcast_ref::<Int64Array>()
                .with_context(|| "casting to Int64Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::UInt8 => {
            let column = column
                .as_any()
                .downcast_ref::<UInt8Array>()
                .with_context(|| "casting to UInt8Array")?;
            encoder.encode_field(&(column.value(value_index) as i16))?;
        }
        arrow::datatypes::DataType::UInt16 => {
            let column = column
                .as_any()
                .downcast_ref::<UInt16Array>()
                .with_context(|| "casting to UInt16Array")?;
            encoder.encode_field(&(column.value(value_index) as i32))?;
        }
        arrow::datatypes::DataType::UInt32 => {
            let column = column
                .as_any()
                .downcast_ref::<UInt32Array>()
                .with_context(|| "casting to UInt32Array")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::UInt64 => {
            let column = column
                .as_any()
                .downcast_ref::<UInt64Array>()
                .with_context(|| "casting to UInt64Array")?;
            encoder.encode_field(&(column.value(value_index) as i64))?;
        }
        arrow::datatypes::DataType::Float16 => {
            let column = column
                .as_any()
                .downcast_ref::<Float16Array>()
                .with_context(|| "casting to Float16Array")?;
            encoder.encode_field(&f32::from(column.value(value_index)))?;
        }
        arrow::datatypes::DataType::Float32 => {
            let column = column
                .as_any()
                .downcast_ref::<Float32Array>()
                .with_context(|| "casting to Float32")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Float64 => {
            let column = column
                .as_any()
                .downcast_ref::<Float64Array>()
                .with_context(|| "casting to Float64")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Timestamp(_time_unit, _opt_time_zone) => {
            let column = column
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .with_context(|| "casting to TimestampNanosecondArray")?;
            encoder.encode_field(&DateTime::from_timestamp_nanos(column.value(value_index)))?;
        }
        arrow::datatypes::DataType::Date32 => bail!("DataType::Date32 not yet implemented"),
        arrow::datatypes::DataType::Date64 => bail!("DataType::Date64 not yet implemented"),
        arrow::datatypes::DataType::Time32(_time_unit) => {
            bail!("DataType::Time32 not yet implemented")
        }
        arrow::datatypes::DataType::Time64(_time_unit) => {
            bail!("DataType::Time64 not yet implemented")
        }
        arrow::datatypes::DataType::Duration(_time_unit) => {
            bail!("DataType::Duration not yet implemented")
        }
        arrow::datatypes::DataType::Interval(_interval_unit) => {
            bail!("DataType::Interval not yet implemented")
        }
        arrow::datatypes::DataType::Binary => bail!("DataType::Binary not yet implemented"),
        arrow::datatypes::DataType::FixedSizeBinary(_) => {
            bail!("DataType::FixedSizeBinary not yet implemented")
        }
        arrow::datatypes::DataType::LargeBinary => {
            bail!("DataType::LargeBinary not yet implemented")
        }
        arrow::datatypes::DataType::BinaryView => bail!("DataType::BinaryView not yet implemented"),
        arrow::datatypes::DataType::Utf8 | arrow::datatypes::DataType::LargeUtf8 => {
            let column = column
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting to StringArray")?;
            encoder.encode_field(&column.value(value_index))?;
        }
        arrow::datatypes::DataType::Utf8View => bail!("DataType::Utf8View not yet implemented"),
        arrow::datatypes::DataType::List(field) => {
            let options = EncoderOptions::default();
            let mut json_encoder = make_encoder(field, &column, &options)?;
            let mut buffer = Vec::with_capacity(1024);
            json_encoder.encode(value_index, &mut buffer);
            let value = ValueEncodedAsText {
                inner: String::from_utf8_lossy(&buffer).into(),
            };
            encoder.encode_field(&value)?;
        }
        arrow::datatypes::DataType::ListView(_field) => {
            bail!("DataType::ListView not yet implemented")
        }
        arrow::datatypes::DataType::FixedSizeList(_field, _) => {
            bail!("DataType::FixedSizeList not yet implemented")
        }
        arrow::datatypes::DataType::LargeList(_field) => {
            bail!("DataType::LargeList not yet implemented")
        }
        arrow::datatypes::DataType::LargeListView(_field) => {
            bail!("DataType::LargeListView not yet implemented")
        }
        arrow::datatypes::DataType::Struct(_fields) => {
            bail!("DataType::Struct not yet implemented")
        }
        arrow::datatypes::DataType::Union(_union_fields, _union_mode) => {
            bail!("DataType::Union not yet implemented")
        }
        arrow::datatypes::DataType::Dictionary(_data_type, _data_type1) => {
            bail!("DataType::Dictionary not yet implemented")
        }
        arrow::datatypes::DataType::Decimal128(_, _) => {
            bail!("DataType::Decimal128 not yet implemented")
        }
        arrow::datatypes::DataType::Decimal256(_, _) => {
            bail!("DataType::Decimal256 not yet implemented")
        }
        arrow::datatypes::DataType::Map(_field, _) => bail!("DataType::Map not yet implemented"),
        arrow::datatypes::DataType::RunEndEncoded(_field, _field1) => {
            bail!("DataType::RunEndEncoded not yet implemented")
        }
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
