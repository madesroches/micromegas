use anyhow::{Context, Result};
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::prelude::*;

use arrow_flight::sql::server::PeekableFlightDataStream;
use arrow_flight::sql::DoPutPreparedStatementResult;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use core::str;
use futures::{stream, Stream, TryStreamExt};
use micromegas::uuid;
use once_cell::sync::Lazy;
use prost::Message;
use std::collections::HashSet;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tonic::metadata::MetadataValue;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::sql::metadata::{
    SqlInfoData, SqlInfoDataBuilder, XdbcTypeInfo, XdbcTypeInfoData, XdbcTypeInfoDataBuilder,
};
use arrow_flight::sql::{
    server::FlightSqlService, ActionBeginSavepointRequest, ActionBeginSavepointResult,
    ActionBeginTransactionRequest, ActionBeginTransactionResult, ActionCancelQueryRequest,
    ActionCancelQueryResult, ActionClosePreparedStatementRequest,
    ActionCreatePreparedStatementRequest, ActionCreatePreparedStatementResult,
    ActionCreatePreparedSubstraitPlanRequest, ActionEndSavepointRequest,
    ActionEndTransactionRequest, Any, CommandGetCatalogs, CommandGetCrossReference,
    CommandGetDbSchemas, CommandGetExportedKeys, CommandGetImportedKeys, CommandGetPrimaryKeys,
    CommandGetSqlInfo, CommandGetTableTypes, CommandGetTables, CommandGetXdbcTypeInfo,
    CommandPreparedStatementQuery, CommandPreparedStatementUpdate, CommandStatementIngest,
    CommandStatementQuery, CommandStatementSubstraitPlan, CommandStatementUpdate, Nullable,
    ProstMessageExt, Searchable, SqlInfo, TicketStatementQuery, XdbcDataType,
};
use arrow_flight::utils::batches_to_flight_data;
use arrow_flight::{
    flight_service_server::FlightService, flight_service_server::FlightServiceServer, Action,
    FlightData, FlightDescriptor, FlightEndpoint, FlightInfo, HandshakeRequest, HandshakeResponse,
    IpcMessage, SchemaAsIpc, Ticket,
};
use arrow_ipc::writer::IpcWriteOptions;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::array::{ArrayRef, RecordBatch};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::error::ArrowError;

macro_rules! status {
    ($desc:expr, $err:expr) => {
        Status::internal(format!("{}: {} at {}:{}", $desc, $err, file!(), line!()))
    };
}

const FAKE_TOKEN: &str = "uuid_token";
const FAKE_HANDLE: &str = "uuid_handle";
const FAKE_UPDATE_RESULT: i64 = 1;

static INSTANCE_SQL_DATA: Lazy<SqlInfoData> = Lazy::new(|| {
    let mut builder = SqlInfoDataBuilder::new();
    // Server information
    builder.append(SqlInfo::FlightSqlServerName, "Example Flight SQL Server");
    builder.append(SqlInfo::FlightSqlServerVersion, "1");
    // 1.3 comes from https://github.com/apache/arrow/blob/f9324b79bf4fc1ec7e97b32e3cce16e75ef0f5e3/format/Schema.fbs#L24
    builder.append(SqlInfo::FlightSqlServerArrowVersion, "1.3");
    builder.build().unwrap()
});

static INSTANCE_XBDC_DATA: Lazy<XdbcTypeInfoData> = Lazy::new(|| {
    let mut builder = XdbcTypeInfoDataBuilder::new();
    builder.append(XdbcTypeInfo {
        type_name: "INTEGER".into(),
        data_type: XdbcDataType::XdbcInteger,
        column_size: Some(32),
        literal_prefix: None,
        literal_suffix: None,
        create_params: None,
        nullable: Nullable::NullabilityNullable,
        case_sensitive: false,
        searchable: Searchable::Full,
        unsigned_attribute: Some(false),
        fixed_prec_scale: false,
        auto_increment: Some(false),
        local_type_name: Some("INTEGER".into()),
        minimum_scale: None,
        maximum_scale: None,
        sql_data_type: XdbcDataType::XdbcInteger,
        datetime_subcode: None,
        num_prec_radix: Some(2),
        interval_precision: None,
    });
    builder.build().unwrap()
});

static TABLES: Lazy<Vec<&'static str>> = Lazy::new(|| vec!["flight_sql.example.table"]);

#[derive(Clone)]
pub struct FlightSqlServiceImpl {}

impl FlightSqlServiceImpl {
    fn check_token<T>(&self, req: &Request<T>) -> Result<(), Status> {
        let metadata = req.metadata();
        let auth = metadata.get("authorization").ok_or_else(|| {
            Status::internal(format!("No authorization header! metadata = {metadata:?}"))
        })?;
        let str = auth
            .to_str()
            .map_err(|e| Status::internal(format!("Error parsing header: {e}")))?;
        let authorization = str.to_string();
        let bearer = "Bearer ";
        if !authorization.starts_with(bearer) {
            Err(Status::internal("Invalid auth header!"))?;
        }
        let token = authorization[bearer.len()..].to_string();
        if token == FAKE_TOKEN {
            Ok(())
        } else {
            Err(Status::unauthenticated("invalid token "))
        }
    }

    fn fake_result() -> Result<RecordBatch, ArrowError> {
        let schema = Schema::new(vec![Field::new("salutation", DataType::Utf8, false)]);
        let mut builder = StringBuilder::new();
        builder.append_value("Hello, FlightSQL!");
        let cols = vec![Arc::new(builder.finish()) as ArrayRef];
        RecordBatch::try_new(Arc::new(schema), cols)
    }
}

#[tonic::async_trait]
impl FlightSqlService for FlightSqlServiceImpl {
    type FlightService = FlightSqlServiceImpl;

    async fn do_handshake(
        &self,
        request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<
        Response<Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>>>,
        Status,
    > {
        let basic = "Basic ";
        let authorization = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::invalid_argument("authorization field not present"))?
            .to_str()
            .map_err(|e| status!("authorization not parsable", e))?;
        if !authorization.starts_with(basic) {
            Err(Status::invalid_argument(format!(
                "Auth type not implemented: {authorization}"
            )))?;
        }
        let base64 = &authorization[basic.len()..];
        let bytes = BASE64_STANDARD
            .decode(base64)
            .map_err(|e| status!("authorization not decodable", e))?;
        let str = str::from_utf8(&bytes).map_err(|e| status!("authorization not parsable", e))?;
        let parts: Vec<_> = str.split(':').collect();
        let (user, pass) = match parts.as_slice() {
            [user, pass] => (user, pass),
            _ => Err(Status::invalid_argument(
                "Invalid authorization header".to_string(),
            ))?,
        };
        if user != &"admin" || pass != &"password" {
            Err(Status::unauthenticated("Invalid credentials!"))?
        }

        let result = HandshakeResponse {
            protocol_version: 0,
            payload: FAKE_TOKEN.into(),
        };
        let result = Ok(result);
        let output = futures::stream::iter(vec![result]);

        let token = format!("Bearer {}", FAKE_TOKEN);
        let mut response: Response<Pin<Box<dyn Stream<Item = _> + Send>>> =
            Response::new(Box::pin(output));
        response.metadata_mut().append(
            "authorization",
            MetadataValue::from_str(token.as_str()).unwrap(),
        );
        return Ok(response);
    }

    async fn do_get_fallback(
        &self,
        request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        self.check_token(&request)?;
        let batch = Self::fake_result().map_err(|e| status!("Could not fake a result", e))?;
        let schema = batch.schema_ref();
        let batches = vec![batch.clone()];
        let flight_data = batches_to_flight_data(schema, batches)
            .map_err(|e| status!("Could not convert batches", e))?
            .into_iter()
            .map(Ok);

        let stream: Pin<Box<dyn Stream<Item = Result<FlightData, Status>> + Send>> =
            Box::pin(stream::iter(flight_data));
        let resp = Response::new(stream);
        Ok(resp)
    }

    async fn get_flight_info_statement(
        &self,
        query: CommandStatementQuery,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        info!("get_flight_info_statement");
        let CommandStatementQuery { query, .. } = query;
        // let ctx = SessionContext::new();
        let schema = Schema::new(vec![Field::new("salutation", DataType::Utf8, false)]);
        let uuid = uuid::Uuid::new_v4();
        let ticket = TicketStatementQuery {
            statement_handle: uuid.to_string().into(),
        };
        let mut bytes: Vec<u8> = Vec::new();
        if ticket.encode(&mut bytes).is_ok() {
            let info = FlightInfo::new()
                .try_with_schema(&schema)
                .unwrap()
                .with_endpoint(FlightEndpoint::new().with_ticket(Ticket::new(bytes)))
                .with_descriptor(FlightDescriptor::new_cmd(query));
            debug!("flight info: {:?}", info);

            Ok(Response::new(info))
        } else {
            error!("Error encoding ticket");
            Err(Status::internal("Error encoding ticket"))
        }
    }

    async fn get_flight_info_substrait_plan(
        &self,
        _query: CommandStatementSubstraitPlan,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_substrait_plan not implemented",
        ))
    }

    async fn get_flight_info_prepared_statement(
        &self,
        cmd: CommandPreparedStatementQuery,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        self.check_token(&request)?;
        let handle = std::str::from_utf8(&cmd.prepared_statement_handle)
            .map_err(|e| status!("Unable to parse handle", e))?;

        let batch = Self::fake_result().map_err(|e| status!("Could not fake a result", e))?;
        let schema = (*batch.schema()).clone();
        let num_rows = batch.num_rows();
        let num_bytes = batch.get_array_memory_size();

        let fetch = FetchResults {
            handle: handle.to_string(),
        };
        let buf = fetch.as_any().encode_to_vec().into();
        let ticket = Ticket { ticket: buf };
        let endpoint = FlightEndpoint {
            ticket: Some(ticket),
            location: vec![],
            expiration_time: None,
            app_metadata: vec![].into(),
        };
        let info = FlightInfo::new()
            .try_with_schema(&schema)
            .map_err(|e| status!("Unable to serialize schema", e))?
            .with_descriptor(FlightDescriptor::new_cmd(vec![]))
            .with_endpoint(endpoint)
            .with_total_records(num_rows as i64)
            .with_total_bytes(num_bytes as i64)
            .with_ordered(false);

        let resp = Response::new(info);
        Ok(resp)
    }

    async fn get_flight_info_catalogs(
        &self,
        query: CommandGetCatalogs,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let flight_descriptor = request.into_inner();
        let ticket = Ticket {
            ticket: query.as_any().encode_to_vec().into(),
        };
        let endpoint = FlightEndpoint::new().with_ticket(ticket);

        let flight_info = FlightInfo::new()
            .try_with_schema(&query.into_builder().schema())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);

        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_schemas(
        &self,
        query: CommandGetDbSchemas,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let flight_descriptor = request.into_inner();
        let ticket = Ticket {
            ticket: query.as_any().encode_to_vec().into(),
        };
        let endpoint = FlightEndpoint::new().with_ticket(ticket);

        let flight_info = FlightInfo::new()
            .try_with_schema(&query.into_builder().schema())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);

        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_tables(
        &self,
        query: CommandGetTables,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let flight_descriptor = request.into_inner();
        let ticket = Ticket {
            ticket: query.as_any().encode_to_vec().into(),
        };
        let endpoint = FlightEndpoint::new().with_ticket(ticket);

        let flight_info = FlightInfo::new()
            .try_with_schema(&query.into_builder().schema())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);

        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_table_types not implemented",
        ))
    }

    async fn get_flight_info_sql_info(
        &self,
        query: CommandGetSqlInfo,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let flight_descriptor = request.into_inner();
        let ticket = Ticket::new(query.as_any().encode_to_vec());
        let endpoint = FlightEndpoint::new().with_ticket(ticket);

        let flight_info = FlightInfo::new()
            .try_with_schema(query.into_builder(&INSTANCE_SQL_DATA).schema().as_ref())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);

        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_primary_keys not implemented",
        ))
    }

    async fn get_flight_info_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_exported_keys not implemented",
        ))
    }

    async fn get_flight_info_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_imported_keys not implemented",
        ))
    }

    async fn get_flight_info_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented(
            "get_flight_info_imported_keys not implemented",
        ))
    }

    async fn get_flight_info_xdbc_type_info(
        &self,
        query: CommandGetXdbcTypeInfo,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let flight_descriptor = request.into_inner();
        let ticket = Ticket::new(query.as_any().encode_to_vec());
        let endpoint = FlightEndpoint::new().with_ticket(ticket);

        let flight_info = FlightInfo::new()
            .try_with_schema(query.into_builder(&INSTANCE_XBDC_DATA).schema().as_ref())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);

        Ok(tonic::Response::new(flight_info))
    }

    // do_get
    async fn do_get_statement(
        &self,
        _ticket: TicketStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("do_get_statement not implemented"))
    }

    async fn do_get_prepared_statement(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented(
            "do_get_prepared_statement not implemented",
        ))
    }

    async fn do_get_catalogs(
        &self,
        query: CommandGetCatalogs,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let catalog_names = TABLES
            .iter()
            .map(|full_name| full_name.split('.').collect::<Vec<_>>()[0].to_string())
            .collect::<HashSet<_>>();
        let mut builder = query.into_builder();
        for catalog_name in catalog_names {
            builder.append(catalog_name);
        }
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_schemas(
        &self,
        query: CommandGetDbSchemas,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let schemas = TABLES
            .iter()
            .map(|full_name| {
                let parts = full_name.split('.').collect::<Vec<_>>();
                (parts[0].to_string(), parts[1].to_string())
            })
            .collect::<HashSet<_>>();

        let mut builder = query.into_builder();
        for (catalog_name, schema_name) in schemas {
            builder.append(catalog_name, schema_name);
        }

        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_tables(
        &self,
        query: CommandGetTables,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let tables = TABLES
            .iter()
            .map(|full_name| {
                let parts = full_name.split('.').collect::<Vec<_>>();
                (
                    parts[0].to_string(),
                    parts[1].to_string(),
                    parts[2].to_string(),
                )
            })
            .collect::<HashSet<_>>();

        let dummy_schema = Schema::empty();
        let mut builder = query.into_builder();
        for (catalog_name, schema_name, table_name) in tables {
            builder
                .append(
                    catalog_name,
                    schema_name,
                    table_name,
                    "TABLE",
                    &dummy_schema,
                )
                .map_err(Status::from)?;
        }

        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("do_get_table_types not implemented"))
    }

    async fn do_get_sql_info(
        &self,
        query: CommandGetSqlInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let builder = query.into_builder(&INSTANCE_SQL_DATA);
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("do_get_primary_keys not implemented"))
    }

    async fn do_get_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented(
            "do_get_exported_keys not implemented",
        ))
    }

    async fn do_get_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented(
            "do_get_imported_keys not implemented",
        ))
    }

    async fn do_get_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented(
            "do_get_cross_reference not implemented",
        ))
    }

    async fn do_get_xdbc_type_info(
        &self,
        query: CommandGetXdbcTypeInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // create a builder with pre-defined Xdbc data:
        let builder = query.into_builder(&INSTANCE_XBDC_DATA);
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    // do_put
    async fn do_put_statement_update(
        &self,
        _ticket: CommandStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        Ok(FAKE_UPDATE_RESULT)
    }

    async fn do_put_statement_ingest(
        &self,
        _ticket: CommandStatementIngest,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        Ok(FAKE_UPDATE_RESULT)
    }

    async fn do_put_substrait_plan(
        &self,
        _ticket: CommandStatementSubstraitPlan,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        Err(Status::unimplemented(
            "do_put_substrait_plan not implemented",
        ))
    }

    async fn do_put_prepared_statement_query(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<DoPutPreparedStatementResult, Status> {
        Err(Status::unimplemented(
            "do_put_prepared_statement_query not implemented",
        ))
    }

    async fn do_put_prepared_statement_update(
        &self,
        _query: CommandPreparedStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        Err(Status::unimplemented(
            "do_put_prepared_statement_update not implemented",
        ))
    }

    async fn do_action_create_prepared_statement(
        &self,
        _query: ActionCreatePreparedStatementRequest,
        request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        self.check_token(&request)?;
        let record_batch =
            Self::fake_result().map_err(|e| status!("Error getting result schema", e))?;
        let schema = record_batch.schema_ref();
        let message = SchemaAsIpc::new(schema, &IpcWriteOptions::default())
            .try_into()
            .map_err(|e| status!("Unable to serialize schema", e))?;
        let IpcMessage(schema_bytes) = message;
        let res = ActionCreatePreparedStatementResult {
            prepared_statement_handle: FAKE_HANDLE.into(),
            dataset_schema: schema_bytes,
            parameter_schema: Default::default(), // TODO: parameters
        };
        Ok(res)
    }

    async fn do_action_close_prepared_statement(
        &self,
        _query: ActionClosePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        Ok(())
    }

    async fn do_action_create_prepared_substrait_plan(
        &self,
        _query: ActionCreatePreparedSubstraitPlanRequest,
        _request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        Err(Status::unimplemented(
            "Implement do_action_create_prepared_substrait_plan",
        ))
    }

    async fn do_action_begin_transaction(
        &self,
        _query: ActionBeginTransactionRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginTransactionResult, Status> {
        Err(Status::unimplemented(
            "Implement do_action_begin_transaction",
        ))
    }

    async fn do_action_end_transaction(
        &self,
        _query: ActionEndTransactionRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        Err(Status::unimplemented("Implement do_action_end_transaction"))
    }

    async fn do_action_begin_savepoint(
        &self,
        _query: ActionBeginSavepointRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginSavepointResult, Status> {
        Err(Status::unimplemented("Implement do_action_begin_savepoint"))
    }

    async fn do_action_end_savepoint(
        &self,
        _query: ActionEndSavepointRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        Err(Status::unimplemented("Implement do_action_end_savepoint"))
    }

    async fn do_action_cancel_query(
        &self,
        _query: ActionCancelQueryRequest,
        _request: Request<Action>,
    ) -> Result<ActionCancelQueryResult, Status> {
        Err(Status::unimplemented("Implement do_action_cancel_query"))
    }

    async fn register_sql_info(&self, _id: i32, _result: &SqlInfo) {}
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FetchResults {
    #[prost(string, tag = "1")]
    pub handle: ::prost::alloc::string::String,
}

impl ProstMessageExt for FetchResults {
    fn type_url() -> &'static str {
        "type.googleapis.com/arrow.flight.protocol.sql.FetchResults"
    }

    fn as_any(&self) -> Any {
        Any {
            type_url: FetchResults::type_url().to_string(),
            value: ::prost::Message::encode_to_vec(self).into(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .build();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    migrate_lakehouse(data_lake.db_pool.clone())
        .await
        .with_context(|| "migrate_lakehouse")?;
    let _view_factory = default_view_factory()?;

    let addr_str = "0.0.0.0:50051";
    let addr = addr_str.parse()?;
    info!("Listening on {:?}", addr);

    let svc = FlightServiceServer::new(FlightSqlServiceImpl {});
    Server::builder().add_service(svc).serve(addr).await?;
    info!("bye");
    Ok(())
}
