mod errors;

pub use errors::{Error, ErrorExt, Result};

use std::{sync::Arc, time::Duration};

use axum::{error_handling::HandleErrorLayer, BoxError, Router};
use http::{header, Method, StatusCode};
use lgn_auth::{
    jwt::{
        signature_validation::{
            BoxedSignatureValidation, NoSignatureValidation, SignatureValidationExt,
        },
        RequestAuthorizer, Validation,
    },
    UserInfo,
};
use tower::ServiceBuilder;
use tower_http::{
    auth::RequireAuthorizationLayer,
    cors::{AllowOrigin, CorsLayer},
};

use crate::Config;

pub struct RouterOptions {
    /// The list of origins that are allowed to make requests, for CORS.
    allow_origin: AllowOrigin,

    /// Whether to allow credentials.
    allow_credentials: bool,

    /// The health function to use to determine service health.
    ///
    /// If this is `None`, a default function returning `true` will be used.
    health_fn: Option<Box<dyn Fn() -> bool + Send + Sync>>,

    /// The ready function to use to determine service readiness.
    ///
    /// Once the ready function returns `true`, the service will be considered
    /// ready.
    ///
    /// If this is `None`, a default function returning `true` will be used.
    ready_fn: Option<Box<dyn Fn() -> bool + Send + Sync>>,

    /// The signature validation function.
    signature_validation: Validation<BoxedSignatureValidation>,
}

impl RouterOptions {
    /// Instantiates a new `RouterOptions` with the default values suitable for
    /// development.
    pub fn new_for_development() -> Self {
        Self {
            allow_origin: AllowOrigin::any(),
            allow_credentials: false,
            health_fn: None,
            ready_fn: None,
            signature_validation: Validation::new(NoSignatureValidation.into_boxed()),
        }
    }

    /// Instantiates a new `RouterOptions` with the specified CORS origins.
    pub async fn new(
        allow_origins: Vec<http::HeaderValue>,
        allow_credentials: bool,
    ) -> crate::Result<Self> {
        let signature_validation = Config::load()?
            .signature_validation
            .instantiate_validation()
            .await?;

        Ok(Self {
            allow_origin: AllowOrigin::list(allow_origins),
            allow_credentials,
            health_fn: None,
            ready_fn: None,
            signature_validation,
        })
    }

    /// Set the health function to use to determine service health.
    ///
    /// When the service is healthy, the function must return `true`.
    ///
    /// The definition of a healthy service is a service that can process
    /// requests successfully.
    pub fn set_health_fn(&mut self, health_fn: impl Fn() -> bool + Send + Sync + 'static) {
        self.health_fn = Some(Box::new(health_fn));
    }

    /// Set the ready function to use to determine service readiness.
    ///
    /// When the service is ready, the function must return `true`.
    ///
    /// The definition of a ready service is a service that can accept requests.
    pub fn set_ready_fn(&mut self, ready_fn: impl Fn() -> bool + Send + Sync + 'static) {
        self.ready_fn = Some(Box::new(ready_fn));
    }
}

#[macro_export]
macro_rules! with_api_server {
    ($router:expr, $api:expr) => {};
}

/// An extension trait that adds convenience methods to `Router`.
pub trait RouterExt: Sized {
    fn apply_router_options(self, options: RouterOptions) -> Router;

    fn apply_development_router_options(self) -> Router {
        self.apply_router_options(RouterOptions::new_for_development())
    }
}

/// Create a new router with the given options.
///
/// This is a convenience method to help create routers that have the
/// appropriate service routes and behavior.
///
/// Namely, it sets up the CORS headers, authentication and adds the health
/// check routes.
impl RouterExt for Router {
    fn apply_router_options(mut self, options: RouterOptions) -> Router {
        // Note: only routes that were added BEFORE the layer will benefit from
        // it.
        // Make sure to register all API routes before calling this function or
        // the routes won't have neither CORS nor authentication!

        let auth = RequireAuthorizationLayer::custom(RequestAuthorizer::<UserInfo, _, _>::new(
            options.signature_validation,
        ));

        self = self.layer(auth);

        let cors = CorsLayer::new()
            .allow_origin(options.allow_origin)
            .allow_credentials(options.allow_credentials)
            .max_age(Duration::from_secs(60 * 60))
            .allow_headers(vec![
                header::ACCEPT,
                header::ACCEPT_LANGUAGE,
                header::AUTHORIZATION,
                header::CONTENT_LANGUAGE,
                header::CONTENT_TYPE,
            ])
            .allow_methods(vec![
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::HEAD,
                Method::OPTIONS,
                Method::CONNECT,
            ]);

        self = self.layer(cors);

        // Add the health check route.
        let health_fn = Arc::new(options.health_fn.unwrap_or_else(|| Box::new(|| true)));

        let health_handler = || async move {
            if health_fn() {
                (StatusCode::OK, [("content-type", "text/plain")], "OK\n")
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("content-type", "text/plain")],
                    "UNAVAILABLE\n",
                )
            }
        };

        self = self.route("/health", axum::routing::get(health_handler));

        // Add the readiness route.
        let ready_fn = Arc::new(options.ready_fn.unwrap_or_else(|| Box::new(|| true)));

        let ready_handler = || async move {
            if ready_fn() {
                (StatusCode::OK, [("content-type", "text/plain")], "READY\n")
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("content-type", "text/plain")],
                    "NOT_READY\n",
                )
            }
        };

        self = self.route("/ready", axum::routing::get(ready_handler));

        self.layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .timeout(Duration::from_secs(5)),
        )
    }
}

#[allow(clippy::unused_async)]
async fn handle_timeout_error(err: BoxError) -> (StatusCode, String) {
    if err.is::<tower::timeout::error::Elapsed>() {
        (
            StatusCode::REQUEST_TIMEOUT,
            "Request took too long\n".to_string(),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal server error: {}\n", err),
        )
    }
}
