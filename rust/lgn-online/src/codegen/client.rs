//! Provides client helper methods for making `gRPC` calls.

use hyper::{client::HttpConnector, Body};
use hyper_rustls::HttpsConnector;

pub type HyperClient = hyper::Client<HttpsConnector<HttpConnector>, Body>;

/// Instantiate a new `OpenApi` client that operates over HTTP1.
pub fn new_hyper_client() -> HyperClient {
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let client = hyper::Client::builder()
        .pool_max_idle_per_host(std::usize::MAX)
        .pool_idle_timeout(None)
        .build(https_connector);

    client
}
