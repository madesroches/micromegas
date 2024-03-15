use futures_util::future::BoxFuture;
use hyper::{Request, Response, StatusCode};
use micromegas_tracing::{debug, warn};
use tower_http::auth::AsyncAuthorizeRequest;

use super::{ApiKey, ApiKeyValidator};

/// An authentication layer that validates JWT tokens and exposes its claims.
pub struct RequestAuthorizer<Validator, ResBody> {
    validator: Validator,
    _phantom: std::marker::PhantomData<ResBody>,
}

impl<Validator: Clone, ResBody> Clone for RequestAuthorizer<Validator, ResBody> {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<Validator, ResBody> RequestAuthorizer<Validator, ResBody>
where
    ResBody: Default,
{
    /// Creates a new `RequestAuthorizer`.
    pub fn new(validator: Validator) -> Self {
        Self {
            validator,
            _phantom: std::marker::PhantomData {},
        }
    }

    fn get_api_key<ReqBody>(request: &mut Request<ReqBody>) -> Result<ApiKey, Response<ResBody>> {
        if let Some(header) = request.headers().get("Authorization") {
            match header.to_str() {
                Ok(authorization) => {
                    let parts = authorization.split_whitespace().collect::<Vec<_>>();

                    if parts.len() != 2 {
                        warn!("Invalid authorization header: expected `Bearer <api-key>`");

                        Err(Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Default::default())
                            .unwrap())
                    } else if parts[0] != "Bearer" {
                        warn!("Invalid authorization header: expected `Bearer <api-key>` but got `{} <...>`", parts[0]);

                        Err(Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Default::default())
                            .unwrap())
                    } else {
                        Ok(ApiKey(parts[1].to_string()))
                    }
                }
                Err(err) => {
                    warn!("Invalid authorization header: {}", err);

                    Err(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(Default::default())
                        .unwrap())
                }
            }
        } else {
            warn!("No authorization header found");

            Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Default::default())
                .unwrap())
        }
    }
}

impl<Validator, ReqBody, ResBody> AsyncAuthorizeRequest<ReqBody>
    for RequestAuthorizer<Validator, ResBody>
where
    Validator: ApiKeyValidator + Clone + Send + Sync + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send,
{
    type RequestBody = ReqBody;
    type ResponseBody = ResBody;
    type Future =
        BoxFuture<'static, Result<Request<Self::RequestBody>, Response<Self::ResponseBody>>>;

    fn authorize(&mut self, mut request: Request<ReqBody>) -> Self::Future {
        let validator = self.validator.clone();

        Box::pin(async move {
            match Self::get_api_key(&mut request) {
                Ok(api_key) => match validator.validate_api_key(api_key).await {
                    Ok(_) => {
                        debug!("API key validation succeeded.");

                        Ok(request)
                    }
                    Err(err) => {
                        warn!("API key validation failed: {}", err);

                        Err(err.into())
                    }
                },
                Err(response) => Err(response),
            }
        })
    }
}
