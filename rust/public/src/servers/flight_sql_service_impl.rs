use super::sqlinfo::{
    SQL_INFO_DATE_TIME_FUNCTIONS, SQL_INFO_NUMERIC_FUNCTIONS, SQL_INFO_SQL_KEYWORDS,
    SQL_INFO_STRING_FUNCTIONS, SQL_INFO_SYSTEM_FUNCTIONS,
};
use anyhow::Result;
use arrow_flight::decode::FlightRecordBatchStream;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::error::FlightError;
use arrow_flight::sql::metadata::{SqlInfoData, SqlInfoDataBuilder};
use arrow_flight::sql::server::PeekableFlightDataStream;
use arrow_flight::sql::DoPutPreparedStatementResult;
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
    CommandStatementQuery, CommandStatementSubstraitPlan, CommandStatementUpdate, ProstMessageExt,
    SqlInfo, TicketStatementQuery,
};
use arrow_flight::{
    flight_service_server::FlightService, Action, FlightDescriptor, FlightEndpoint, FlightInfo,
    HandshakeRequest, HandshakeResponse, Ticket,
};
use core::str;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::ipc::writer::StreamWriter;
use datafusion::execution::runtime_env::RuntimeEnv;
use futures::StreamExt;
use futures::{Stream, TryStreamExt};
use micromegas_analytics::lakehouse::partition_cache::QueryPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::replication::bulk_ingest;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use once_cell::sync::Lazy;
use prost::Message;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};

macro_rules! status {
    ($desc:expr, $err:expr) => {
        Status::internal(format!("{}: {} at {}:{}", $desc, $err, file!(), line!()))
    };
}

macro_rules! api_entry_not_implemented {
    () => {{
        let function_name = micromegas_tracing::__function_name!();
        error!("not implemented: {function_name}");
        Err(Status::unimplemented(format!(
            "{}:{} not implemented: {function_name}",
            file!(),
            line!()
        )))
    }};
}

static INSTANCE_SQL_DATA: Lazy<SqlInfoData> = Lazy::new(|| {
    let mut builder = SqlInfoDataBuilder::new();
    // Server information
    builder.append(SqlInfo::FlightSqlServerName, "Micromegas Flight SQL Server");
    builder.append(SqlInfo::FlightSqlServerVersion, "1");
    // 1.3 comes from https://github.com/apache/arrow/blob/f9324b79bf4fc1ec7e97b32e3cce16e75ef0f5e3/format/Schema.fbs#L24
    builder.append(SqlInfo::FlightSqlServerArrowVersion, "1.3");
    builder.append(SqlInfo::SqlKeywords, SQL_INFO_SQL_KEYWORDS);
    builder.append(SqlInfo::SqlNumericFunctions, SQL_INFO_NUMERIC_FUNCTIONS);
    builder.append(SqlInfo::SqlStringFunctions, SQL_INFO_STRING_FUNCTIONS);
    builder.append(SqlInfo::SqlSystemFunctions, SQL_INFO_SYSTEM_FUNCTIONS);
    builder.append(SqlInfo::SqlDatetimeFunctions, SQL_INFO_DATE_TIME_FUNCTIONS);
    builder.build().unwrap()
});

#[derive(Clone)]
pub struct FlightSqlServiceImpl {
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    view_factory: Arc<ViewFactory>,
}

impl FlightSqlServiceImpl {
    pub fn new(
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        view_factory: Arc<ViewFactory>,
    ) -> Result<Self> {
        Ok(Self {
            runtime,
            lake,
            part_provider,
            view_factory,
        })
    }

    async fn execute_query(
        &self,
        ticket_stmt: TicketStatementQuery,
        metadata: &MetadataMap,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let begin_request = now();
        let sql = std::str::from_utf8(&ticket_stmt.statement_handle)
            .map_err(|e| status!("Unable to parse query", e))?;

        let mut begin = metadata.get("query_range_begin");
        if let Some(s) = &begin {
            if s.is_empty() {
                begin = None;
            }
        }
        let mut end = metadata.get("query_range_end");
        if let Some(s) = &end {
            if s.is_empty() {
                end = None;
            }
        }
        let query_range = if begin.is_some() && end.is_some() {
            let begin_datetime = chrono::DateTime::parse_from_rfc3339(
                begin
                    .unwrap()
                    .to_str()
                    .map_err(|e| status!("Unable to convert query_range_begin to string", e))?,
            )
            .map_err(|e| status!("Unable to parse query_range_begin as a rfc3339 datetime", e))?;
            let end_datetime = chrono::DateTime::parse_from_rfc3339(
                end.unwrap()
                    .to_str()
                    .map_err(|e| status!("Unable to convert query_range_end to string", e))?,
            )
            .map_err(|e| status!("Unable to parse query_range_end as a rfc3339 datetime", e))?;
            Some(TimeRange::new(begin_datetime.into(), end_datetime.into()))
        } else {
            None
        };

        info!(
            "execute_query range={query_range:?} sql={sql:?} limit={:?}",
            metadata.get("limit")
        );
        let ctx = make_session_context(
            self.runtime.clone(),
            self.lake.clone(),
            self.part_provider.clone(),
            query_range,
            self.view_factory.clone(),
        )
        .await
        .map_err(|e| status!("error in make_session_context", e))?;
        let mut df = ctx
            .sql(sql)
            .await
            .map_err(|e| status!("error building dataframe", e))?;

        if let Some(limit_str) = metadata.get("limit") {
            let limit: usize = usize::from_str(
                limit_str
                    .to_str()
                    .map_err(|e| status!("error converting limit to str", e))?,
            )
            .map_err(|e| status!("error parsing limit", e))?;
            df = df
                .limit(0, Some(limit))
                .map_err(|e| status!("error building dataframe with limit", e))?;
        }
        let schema = Arc::new(df.schema().as_arrow().clone());
        let stream = df
            .execute_stream()
            .await
            .map_err(|e| Status::internal(format!("Error executing plan: {e:?}")))?
            .map_err(|e| FlightError::ExternalError(Box::new(e)));
        let builder = FlightDataEncoderBuilder::new().with_schema(schema);
        let flight_data_stream = builder.build(stream);
        let boxed_flight_stream = flight_data_stream
            .map_err(|e| status!("error building data stream", e))
            .boxed();
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(Response::new(boxed_flight_stream))
    }
}

#[tonic::async_trait]
impl FlightSqlService for FlightSqlServiceImpl {
    type FlightService = FlightSqlServiceImpl;

    async fn do_handshake(
        &self,
        _request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<
        Response<Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>>>,
        Status,
    > {
        api_entry_not_implemented!()
    }

    async fn do_get_fallback(
        &self,
        request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let ticket_stmt = TicketStatementQuery::decode(request.get_ref().ticket.clone())
            .map_err(|e| status!("Could not read ticket", e))?;
        self.execute_query(ticket_stmt, request.metadata()).await
    }

    async fn get_flight_info_statement(
        &self,
        query: CommandStatementQuery,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_statement {query:?} ");
        let CommandStatementQuery { query, .. } = query;
        let schema = Schema::empty();
        let ticket = TicketStatementQuery {
            statement_handle: query.into(),
        };
        let mut bytes: Vec<u8> = Vec::new();
        if ticket.encode(&mut bytes).is_ok() {
            let info = FlightInfo::new()
                .try_with_schema(&schema)
                .unwrap()
                .with_endpoint(FlightEndpoint::new().with_ticket(Ticket::new(bytes)));
            let duration = now() - begin_request;
            imetric!("request_duration", "ticks", duration as u64);
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
        api_entry_not_implemented!()
    }

    async fn get_flight_info_prepared_statement(
        &self,
        _cmd: CommandPreparedStatementQuery,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_catalogs(
        &self,
        _query: CommandGetCatalogs,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_schemas(
        &self,
        _query: CommandGetDbSchemas,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_tables(
        &self,
        query: CommandGetTables,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_tables");
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
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_sql_info(
        &self,
        query: CommandGetSqlInfo,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_sql_info");
        let flight_descriptor = request.into_inner();
        let ticket = Ticket::new(query.as_any().encode_to_vec());
        let endpoint = FlightEndpoint::new().with_ticket(ticket);
        let flight_info = FlightInfo::new()
            .try_with_schema(query.into_builder(&INSTANCE_SQL_DATA).schema().as_ref())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_xdbc_type_info(
        &self,
        _query: CommandGetXdbcTypeInfo,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_statement(
        &self,
        ticket: TicketStatementQuery,
        request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        self.execute_query(ticket, request.metadata()).await
    }

    async fn do_get_prepared_statement(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_catalogs(
        &self,
        _query: CommandGetCatalogs,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_schemas(
        &self,
        _query: CommandGetDbSchemas,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_tables(
        &self,
        query: CommandGetTables,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let begin_request = now();
        info!("do_get_tables {query:?}");
        let mut builder = query.into_builder();
        for view in self.view_factory.get_global_views() {
            let catalog_name = "";
            let schema_name = "";
            builder
                .append(
                    catalog_name,
                    schema_name,
                    &*view.get_view_set_name(),
                    "table",
                    &view.get_file_schema(),
                )
                .map_err(Status::from)?;
        }
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_sql_info(
        &self,
        query: CommandGetSqlInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        info!("do_get_sql_info");
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
        api_entry_not_implemented!()
    }

    async fn do_get_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_xdbc_type_info(
        &self,
        _query: CommandGetXdbcTypeInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_statement_update(
        &self,
        _ticket: CommandStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_statement_ingest(
        &self,
        command: CommandStatementIngest,
        request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        let table_name = command.table;
        info!("do_put_statement_ingest table_name={table_name}");
        let stream = FlightRecordBatchStream::new_from_flight_data(
            request.into_inner().map_err(|e| e.into()),
        );
        bulk_ingest(self.lake.clone(), &table_name, stream)
            .await
            .map_err(|e| {
                let msg = format!("error ingesting into {table_name}: {e:?}");
                error!("{msg}");
                status!(msg, e)
            })
    }

    async fn do_put_substrait_plan(
        &self,
        _ticket: CommandStatementSubstraitPlan,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_prepared_statement_query(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<DoPutPreparedStatementResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_prepared_statement_update(
        &self,
        _query: CommandPreparedStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_create_prepared_statement(
        &self,
        query: ActionCreatePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        info!("do_action_create_prepared_statement query={}", &query.query);

        let ctx = make_session_context(
            self.runtime.clone(),
            self.lake.clone(),
            self.part_provider.clone(),
            None,
            self.view_factory.clone(),
        )
        .await
        .map_err(|e| status!("error in make_session_context", e))?;
        let df = ctx
            .sql(&query.query)
            .await
            .map_err(|e| status!("error building dataframe", e))?;
        let schema = df.schema().as_arrow();
        let mut schema_buffer = Vec::new();
        let mut writer = StreamWriter::try_new(&mut schema_buffer, schema)
            .map_err(|e| status!("error writing schema to in-memory buffer", e))?;
        writer
            .finish()
            .map_err(|e| status!("error closing arrow ipc stream writer", e))?;
        // here we could serialize the logical plan and return that as the prepared statement, but we would
        // need to register LogicalExtensionCodec for user-defined functions
        // instead, we are sending back the sql as we received it
        let result = ActionCreatePreparedStatementResult {
            prepared_statement_handle: query.query.into(),
            dataset_schema: schema_buffer.into(),
            parameter_schema: "".into(),
        };
        Ok(result)
    }

    async fn do_action_close_prepared_statement(
        &self,
        _query: ActionClosePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        info!("do_action_close_prepared_statement");
        Ok(())
    }

    async fn do_action_create_prepared_substrait_plan(
        &self,
        _query: ActionCreatePreparedSubstraitPlanRequest,
        _request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_begin_transaction(
        &self,
        _query: ActionBeginTransactionRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginTransactionResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_end_transaction(
        &self,
        _query: ActionEndTransactionRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_begin_savepoint(
        &self,
        _query: ActionBeginSavepointRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginSavepointResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_end_savepoint(
        &self,
        _query: ActionEndSavepointRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_cancel_query(
        &self,
        _query: ActionCancelQueryRequest,
        _request: Request<Action>,
    ) -> Result<ActionCancelQueryResult, Status> {
        api_entry_not_implemented!()
    }

    async fn register_sql_info(&self, _id: i32, _result: &SqlInfo) {
        info!("register_sql_info");
    }
}
