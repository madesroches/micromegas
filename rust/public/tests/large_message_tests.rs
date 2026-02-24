use arrow_flight::{
    FlightData, FlightDescriptor, FlightEndpoint, FlightInfo, Ticket,
    flight_service_server::{FlightService, FlightServiceServer},
};
use datafusion::arrow::{
    array::{RecordBatch, StringArray},
    datatypes::{DataType, Field, Schema},
};
use futures::stream::BoxStream;
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming, transport::Server};

/// A minimal Flight server that returns a large RecordBatch (>4MB)
/// to test that the client can handle messages exceeding the default
/// gRPC 4MB limit.
struct LargeResponseFlightService;

impl LargeResponseFlightService {
    fn make_large_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("data", DataType::Utf8, false)]));
        // Each row is ~1KB, 5000 rows = ~5MB (exceeds 4MB default limit)
        let large_string = "x".repeat(1024);
        let values: Vec<&str> = (0..5000).map(|_| large_string.as_str()).collect();
        let array = StringArray::from(values);
        RecordBatch::try_new(schema, vec![Arc::new(array)]).expect("creating large record batch")
    }
}

#[tonic::async_trait]
impl FlightService for LargeResponseFlightService {
    type HandshakeStream = BoxStream<'static, Result<arrow_flight::HandshakeResponse, Status>>;
    type ListFlightsStream = BoxStream<'static, Result<FlightInfo, Status>>;
    type DoGetStream = BoxStream<'static, Result<FlightData, Status>>;
    type DoPutStream = BoxStream<'static, Result<arrow_flight::PutResult, Status>>;
    type DoActionStream = BoxStream<'static, Result<arrow_flight::Result, Status>>;
    type ListActionsStream = BoxStream<'static, Result<arrow_flight::ActionType, Status>>;
    type DoExchangeStream = BoxStream<'static, Result<FlightData, Status>>;

    async fn get_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let batch = Self::make_large_batch();
        let schema = batch.schema();

        let ticket = Ticket::new(b"large-data".to_vec());
        let endpoint = FlightEndpoint::new().with_ticket(ticket);
        let info = FlightInfo::new()
            .try_with_schema(schema.as_ref())
            .map_err(|e| Status::internal(format!("{e}")))?
            .with_endpoint(endpoint);
        Ok(Response::new(info))
    }

    async fn poll_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<arrow_flight::PollInfo>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn get_schema(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<arrow_flight::SchemaResult>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn do_get(
        &self,
        _request: Request<Ticket>,
    ) -> Result<Response<Self::DoGetStream>, Status> {
        let batch = Self::make_large_batch();
        let schema = batch.schema();

        let all_data = arrow_flight::utils::batches_to_flight_data(schema.as_ref(), vec![batch])
            .map_err(|e| Status::internal(format!("{e}")))?;

        let stream = futures::stream::iter(all_data.into_iter().map(Ok));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn handshake(
        &self,
        _request: Request<Streaming<arrow_flight::HandshakeRequest>>,
    ) -> Result<Response<Self::HandshakeStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn list_flights(
        &self,
        _request: Request<arrow_flight::Criteria>,
    ) -> Result<Response<Self::ListFlightsStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn do_put(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoPutStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn do_action(
        &self,
        _request: Request<arrow_flight::Action>,
    ) -> Result<Response<Self::DoActionStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn list_actions(
        &self,
        _request: Request<arrow_flight::Empty>,
    ) -> Result<Response<Self::ListActionsStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }

    async fn do_exchange(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoExchangeStream>, Status> {
        Err(Status::unimplemented("not needed for test"))
    }
}

/// Start the mock server, returning the address it is listening on.
async fn start_server() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding listener");
    let addr = listener.local_addr().expect("getting local addr");

    tokio::spawn(async move {
        let stream = async_stream::stream! {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => yield Ok(stream),
                    Err(e) => yield Err(e),
                }
            }
        };
        Server::builder()
            .add_service(FlightServiceServer::new(LargeResponseFlightService))
            .serve_with_incoming(stream)
            .await
            .expect("server failed");
    });

    // Give the server a moment to start accepting connections
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    addr
}

#[tokio::test]
async fn test_client_handles_large_messages() {
    let addr = start_server().await;

    let channel =
        tonic::transport::Channel::builder(format!("http://{addr}").parse().expect("parsing uri"))
            .connect()
            .await
            .expect("connecting to server");

    let mut client = micromegas::client::flightsql_client::Client::new(channel);
    let batches = client
        .query("SELECT 1".to_string(), None)
        .await
        .expect("query with large response should succeed");

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_rows(), 5000);
}
