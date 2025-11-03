use http::Request;
use std::future::Future;
use std::pin::Pin;
use tonic::Status;
use tower::Service;

/// A `tower::Service` that short circuits the other services when the caller is asking for a health check
#[derive(Clone)]
pub struct GrpcHealthService<S> {
    service: S,
}

impl<S> GrpcHealthService<S> {
    pub const fn new(service: S) -> Self {
        Self { service }
    }
}

impl<S, Body> Service<http::Request<Body>> for GrpcHealthService<S>
where
    Body: Send + 'static + Default,
    S: Service<http::Request<Body>, Response = http::Response<Body>> + Clone + Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
    S::Future: Send,
{
    type Response = http::Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx).map(|_| Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut service = self.service.clone();
        Box::pin(async move {
            if req.uri().path().ends_with("/health") {
                let status = Status::ok("health is ok");
                let response = status.into_http().map(|_: Body| Body::default());
                return Ok(response);
            }
            service.call(req).await
        })
    }
}
