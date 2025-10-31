use micromegas_tracing::info;
use std::task::Context;
use std::task::Poll;
use tower::Service;

use super::http_utils::get_client_ip;

/// A Tower service that logs the URI of incoming requests.
#[derive(Clone)]
pub struct LogUriService<S> {
    /// The inner service to which requests will be forwarded.
    pub service: S,
}

impl<S, Body> Service<http::Request<Body>> for LogUriService<S>
where
    S: Service<http::Request<Body>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    /// Logs the URI of the incoming request and then calls the inner service.
    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let client_ip = get_client_ip(request.headers(), request.extensions());
        info!("uri={:?} client_ip={client_ip}", request.uri());
        self.service.call(request)
    }
}
