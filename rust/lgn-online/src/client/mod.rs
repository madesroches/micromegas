mod authenticated_client;
mod errors;

pub use authenticated_client::AuthenticatedClient;
pub use errors::{Error, Result};

use std::task::{Context, Poll};

use http::{Request, Response};
use tower::Service;

/// A generic `OpenApi` client.
#[derive(Clone, Debug)]
pub struct GenericApiClient<C> {
    inner: C,
}

impl<C, ReqBody, ResBody> Service<Request<ReqBody>> for GenericApiClient<C>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = C::Response;
    type Error = C::Error;
    type Future = C::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        self.inner.call(req)
    }
}

// A default `Hyper` client.
pub type HyperClient = GenericApiClient<
    hyper::Client<hyper_rustls::HttpsConnector<hyper::client::HttpConnector>, hyper::Body>,
>;

impl Default for HyperClient {
    fn default() -> Self {
        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http2()
            .build();
        let client = hyper::Client::builder()
            .pool_max_idle_per_host(std::usize::MAX)
            .pool_idle_timeout(None)
            .build(https_connector);

        Self { inner: client }
    }
}
