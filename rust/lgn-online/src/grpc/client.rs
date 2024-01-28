//! Provides client helper methods for making `gRPC` calls.

use std::task::{Context, Poll};

use http::{Request, Response, Uri};
use hyper::client::HttpConnector;
use hyper_rustls::HttpsConnector;
use tonic::{body::BoxBody, codegen::StdError};
use tower::Service;

use super::web::client::GrpcWebClient as GrpcWebClientImpl;

/// A `gRPC` generic client.
#[derive(Clone, Debug)]
pub struct GenericGrpcClient<C> {
    inner: C,
    uri: Uri,
}

pub type GrpcClient = GenericGrpcClient<hyper::Client<HttpsConnector<HttpConnector>, BoxBody>>;

impl GrpcClient {
    /// Instantiate a new `gRPC` client that operates over a HTTP 2 connection.
    ///
    /// Such a client cannot call `gRPC` servers that operate over HTTP 1 or are
    /// behind a non-HTTP 2 compatible proxy.
    pub fn new(uri: Uri) -> Self {
        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http2()
            .build();
        let client = hyper::Client::builder()
            .pool_max_idle_per_host(std::usize::MAX)
            .pool_idle_timeout(None)
            .http2_only(true)
            .build(https_connector);

        Self { inner: client, uri }
    }
}

pub type GrpcWebClient =
    GenericGrpcClient<GrpcWebClientImpl<hyper::Client<HttpsConnector<HttpConnector>, BoxBody>>>;

impl GrpcWebClient {
    /// Instantiate a new `gRPC` client that operates over a HTTP 1.1 connection
    /// using `gRPC-Web`.
    ///
    /// The client expects the remote server to understand the `gRPC-Web`
    /// protocol.
    pub fn new(uri: Uri) -> Self {
        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http2()
            .build();
        let client = hyper::Client::builder()
            .pool_max_idle_per_host(std::usize::MAX)
            .pool_idle_timeout(None)
            .http2_only(false)
            .build(https_connector);
        let client = GrpcWebClientImpl::new(client);

        Self { inner: client, uri }
    }
}

impl<C, ReqBody, ResBody> Service<Request<ReqBody>> for GenericGrpcClient<C>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
    C::Error: Into<StdError>,
    ResBody: http_body::Body + Send + 'static,
    <ResBody as http_body::Body>::Error: Into<StdError> + Send,
{
    type Response = C::Response;
    type Error = C::Error;
    type Future = C::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let req = prepare_grpc_request(req, &self.uri);
        self.inner.call(req)
    }
}

fn prepare_grpc_request<T>(mut req: Request<T>, uri: &Uri) -> Request<T> {
    let uri = hyper::Uri::builder()
        .scheme(uri.scheme().unwrap().clone())
        .authority(uri.authority().unwrap().clone())
        .path_and_query(req.uri().path_and_query().unwrap().clone())
        .build()
        .unwrap();

    *req.uri_mut() = uri;

    req
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_prepare_grpc_request() {
        let uri = hyper::Uri::from_static("http://127.0.0.1:50051");
        let mut req = hyper::Request::new(42);
        *req.uri_mut() = hyper::Uri::from_static(
            "https://user:password@host:port/foo/bar?query=string#fragment",
        );

        // The scheme, authority and path+query should be taken from the second
        // argument.
        let req = prepare_grpc_request(req, &uri);

        assert_eq!(
            req.uri().to_string(),
            "http://127.0.0.1:50051/foo/bar?query=string"
        );
    }
}
