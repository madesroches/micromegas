use lgn_telemetry_proto::health::health_server::Health;
use lgn_telemetry_proto::health::HealthCheckRequest;
use lgn_telemetry_proto::health::HealthCheckResponse;
use tonic::Response;

pub struct HealthCheckService {}

#[tonic::async_trait]
impl Health for HealthCheckService {
    async fn check(
        &self,
        _request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<HealthCheckResponse>, tonic::Status> {
        Ok(Response::new(HealthCheckResponse { status: 1 }))
    }
}
