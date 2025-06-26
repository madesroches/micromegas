use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Sink;
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
        arrow::datatypes::DataType::Null => todo!(),
        arrow::datatypes::DataType::Boolean => todo!(),
        arrow::datatypes::DataType::Int8 => todo!(),
        arrow::datatypes::DataType::Int16 => todo!(),
        arrow::datatypes::DataType::Int32 => todo!(),
        arrow::datatypes::DataType::Int64 => Ok(Type::INT8),
        arrow::datatypes::DataType::UInt8 => todo!(),
        arrow::datatypes::DataType::UInt16 => todo!(),
        arrow::datatypes::DataType::UInt32 => todo!(),
        arrow::datatypes::DataType::UInt64 => todo!(),
        arrow::datatypes::DataType::Float16 => todo!(),
        arrow::datatypes::DataType::Float32 => todo!(),
        arrow::datatypes::DataType::Float64 => todo!(),
        arrow::datatypes::DataType::Timestamp(_time_unit, _) => todo!(),
        arrow::datatypes::DataType::Date32 => todo!(),
        arrow::datatypes::DataType::Date64 => todo!(),
        arrow::datatypes::DataType::Time32(_time_unit) => todo!(),
        arrow::datatypes::DataType::Time64(_time_unit) => todo!(),
        arrow::datatypes::DataType::Duration(_time_unit) => todo!(),
        arrow::datatypes::DataType::Interval(_interval_unit) => todo!(),
        arrow::datatypes::DataType::Binary => todo!(),
        arrow::datatypes::DataType::FixedSizeBinary(_) => todo!(),
        arrow::datatypes::DataType::LargeBinary => todo!(),
        arrow::datatypes::DataType::BinaryView => todo!(),
        arrow::datatypes::DataType::Utf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::LargeUtf8 => Ok(Type::TEXT),
        arrow::datatypes::DataType::Utf8View => Ok(Type::TEXT),
        arrow::datatypes::DataType::List(_field) => todo!(),
        arrow::datatypes::DataType::ListView(_field) => todo!(),
        arrow::datatypes::DataType::FixedSizeList(_field, _) => todo!(),
        arrow::datatypes::DataType::LargeList(_field) => todo!(),
        arrow::datatypes::DataType::LargeListView(_field) => todo!(),
        arrow::datatypes::DataType::Struct(_fields) => todo!(),
        arrow::datatypes::DataType::Union(_union_fields, _union_mode) => todo!(),
        arrow::datatypes::DataType::Dictionary(_data_type, _data_type1) => todo!(),
        arrow::datatypes::DataType::Decimal128(_, _) => todo!(),
        arrow::datatypes::DataType::Decimal256(_, _) => todo!(),
        arrow::datatypes::DataType::Map(_field, _) => todo!(),
        arrow::datatypes::DataType::RunEndEncoded(_field, _field1) => todo!(),
    }
}

fn arrow_to_pg_schema(
    arrow_schema: &arrow::datatypes::Schema,
) -> anyhow::Result<Arc<Vec<FieldInfo>>> {
    let mut fields_it = arrow_schema.fields().iter();
    let mut res = vec![];
    while let Some(f) = fields_it.next() {
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
        // while let Some(batch_res) = record_batch_stream.next().await {
        //     let batch = batch_res.map_err(|e| PgWireError::ApiError(e.into()))?;
        // }

        // Ok(vec![])

        // let data = vec![
        //     (Some(0), Some("Tom")),
        //     (Some(1), Some("Jerry")),
        //     (Some(2), None),
        // ];
        // let schema_ref = schema.clone();
        // let data_row_stream = stream::iter(data.into_iter()).map(move |r| {
        //     let mut encoder = DataRowEncoder::new(schema_ref.clone());
        //     encoder.encode_field(&r.0)?;
        //     encoder.encode_field(&r.1)?;

        //     encoder.finish()
        // });

        let data_row_stream = async_stream::try_stream! {
            loop{
		for _row_index in 0..record_batch.num_rows() {
                    let mut encoder = DataRowEncoder::new(schema.clone());
                    for _column in record_batch.columns() {
			let value = Some(0);
			encoder.encode_field(&value).map_err(|e| PgWireError::ApiError(e.into()))?;
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
        };

        Ok(vec![Response::Query(QueryResponse::new(
            schema_copy,
            Box::pin(data_row_stream),
        ))])
    }
}
