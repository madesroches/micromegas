use super::flightsql_client::Client;
use anyhow::{Context, Result};
use async_trait::async_trait;
use http::Uri;
use tonic::transport::{Channel, ClientTlsConfig};

/// A trait for creating FlightSQL clients.
#[async_trait]
pub trait FlightSQLClientFactory: Send + Sync {
    async fn make_client(&self) -> Result<Client>;
}

/// A FlightSQL client factory that uses a bearer token for authentication.
pub struct BearerFlightSQLClientFactory {
    url: String,
    token: String,
    client_type: Option<String>,
    extra_metadata: Vec<(String, String)>,
}

impl BearerFlightSQLClientFactory {
    /// Creates a new `BearerFlightSQLClientFactory`.
    ///
    /// # Arguments
    ///
    /// * `url` - The FlightSQL server URL.
    /// * `token` - The bearer token to use for authentication.
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            client_type: None,
            extra_metadata: Vec::new(),
        }
    }

    /// Creates a new `BearerFlightSQLClientFactory` with a specific client type identifier.
    ///
    /// # Arguments
    ///
    /// * `url` - The FlightSQL server URL.
    /// * `token` - The bearer token to use for authentication.
    /// * `client_type` - The client type identifier (e.g., "web", "cli", "python").
    pub fn new_with_client_type(url: String, token: String, client_type: String) -> Self {
        Self {
            url,
            token,
            client_type: Some(client_type),
            extra_metadata: Vec::new(),
        }
    }

    /// Creates a new `BearerFlightSQLClientFactory` that reads the URL from the
    /// `MICROMEGAS_FLIGHTSQL_URL` environment variable.
    pub fn from_env(token: String) -> Result<Self> {
        let url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?;
        Ok(Self {
            url,
            token,
            client_type: None,
            extra_metadata: Vec::new(),
        })
    }

    /// Creates a new `BearerFlightSQLClientFactory` that reads the URL from the
    /// `MICROMEGAS_FLIGHTSQL_URL` environment variable, with a client type.
    pub fn from_env_with_client_type(token: String, client_type: String) -> Result<Self> {
        let url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?;
        Ok(Self {
            url,
            token,
            client_type: Some(client_type),
            extra_metadata: Vec::new(),
        })
    }

    /// Attaches an additional gRPC metadata header sent with every request made by
    /// clients from this factory (e.g., the web app's notebook/cell origin labels).
    /// `key` must already be a valid lowercase gRPC metadata key.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_metadata.push((key.into(), value.into()));
        self
    }
}

/// Rewrites the `grpc://`/`grpc+tls://` scheme convention used by data source configs into the
/// `http://`/`https://` scheme tonic's `Channel` expects for its TLS decision. `http://`/`https://`
/// URLs pass through unchanged.
pub fn normalize_channel_scheme(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("grpc+tls://") {
        format!("https://{}", &url["grpc+tls://".len()..])
    } else if lower.starts_with("grpc://") {
        format!("http://{}", &url["grpc://".len()..])
    } else {
        url.to_string()
    }
}

#[async_trait]
impl FlightSQLClientFactory for BearerFlightSQLClientFactory {
    async fn make_client(&self) -> Result<Client> {
        let normalized_url = normalize_channel_scheme(&self.url);
        let flight_url = normalized_url
            .parse::<Uri>()
            .with_context(|| "parsing flightsql url")?;
        let mut endpoint = Channel::builder(flight_url.clone());
        if flight_url.scheme_str() == Some("https") {
            let tls_config = ClientTlsConfig::new().with_native_roots();
            endpoint = endpoint
                .tls_config(tls_config)
                .with_context(|| "tls_config")?;
        }
        let channel = endpoint
            .connect()
            .await
            .with_context(|| "connecting grpc channel")?;
        let mut client = Client::new(channel);
        let auth_value = if self.token.starts_with("Bearer ") {
            self.token.clone()
        } else {
            format!("Bearer {}", self.token)
        };

        client
            .inner_mut()
            .set_header(http::header::AUTHORIZATION.as_str(), auth_value);

        // Set client type header if provided
        if let Some(client_type) = &self.client_type {
            client
                .inner_mut()
                .set_header("x-client-type", client_type.clone());
        }

        // Set any additional per-factory metadata (e.g., notebook/cell origin labels)
        for (key, value) in &self.extra_metadata {
            client.inner_mut().set_header(key, value.clone());
        }

        // Preserve dictionary encoding for bandwidth efficiency
        client.inner_mut().set_header("preserve_dictionary", "true");

        Ok(client)
    }
}
