use std::{marker::PhantomData, sync::Arc};

use hyper::{Request, Response, StatusCode};
use tower_http::auth::AuthorizeRequest;
use tracing::debug;

use super::{
    signature_validation::{NoSignatureValidation, SignatureValidation},
    Token, Validation,
};

/// An authentication layer that validates JWT tokens and exposes its claims.
pub struct RequestAuthorizer<Claims, SV, ResBody> {
    validation: Arc<Validation<SV>>,
    _phantom: PhantomData<(Claims, ResBody)>,
}

impl<Claims, SV, ResBody> Clone for RequestAuthorizer<Claims, SV, ResBody> {
    fn clone(&self) -> Self {
        Self {
            validation: Arc::clone(&self.validation),
            _phantom: PhantomData,
        }
    }
}

impl<Claims, ResBody> Default for RequestAuthorizer<Claims, NoSignatureValidation, ResBody> {
    fn default() -> Self {
        Self {
            validation: Arc::new(Validation::default()),
            _phantom: PhantomData,
        }
    }
}

impl<Claims, SV, ResBody> RequestAuthorizer<Claims, SV, ResBody>
where
    ResBody: Default,
{
    pub fn new(validation: Validation<SV>) -> Self {
        Self {
            validation: Arc::new(validation),
            _phantom: PhantomData,
        }
    }

    fn get_token<ReqBody>(request: &mut Request<ReqBody>) -> Result<Token<'_>, Response<ResBody>> {
        if let Some(header) = request.headers().get("Authorization") {
            match header.to_str() {
                Ok(authorization) => {
                    let parts = authorization.split_whitespace().collect::<Vec<_>>();

                    if parts.len() != 2 {
                        debug!("Invalid authorization header: expected `Bearer <jwt>`");

                        Err(Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(ResBody::default())
                            .unwrap())
                    } else if parts[0] != "Bearer" {
                        debug!("Invalid authorization header: expected `Bearer <jwt>` but got `{} <...>`", parts[0]);

                        Err(Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(ResBody::default())
                            .unwrap())
                    } else {
                        match Token::try_from(parts[1]) {
                            Ok(token) => Ok(token),
                            Err(err) => {
                                debug!(
                                    "Invalid authorization header: could not parse JWT token: {}",
                                    err
                                );

                                Err(Response::builder()
                                    .status(StatusCode::UNAUTHORIZED)
                                    .body(ResBody::default())
                                    .unwrap())
                            }
                        }
                    }
                }
                Err(err) => {
                    debug!("Invalid authorization header: {}", err);

                    Err(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(ResBody::default())
                        .unwrap())
                }
            }
        } else {
            debug!("No authorization header found");

            Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(ResBody::default())
                .unwrap())
        }
    }
}

impl<Claims, SV, ReqBody, ResBody> AuthorizeRequest<ReqBody>
    for RequestAuthorizer<Claims, SV, ResBody>
where
    Claims: serde::de::DeserializeOwned + Send + Sync + 'static,
    SV: SignatureValidation,
    ResBody: Default,
{
    type ResponseBody = ResBody;

    fn authorize(
        &mut self,
        request: &mut Request<ReqBody>,
    ) -> Result<(), Response<Self::ResponseBody>> {
        let token = Self::get_token(request)?;

        match token.into_claims::<Claims, _>(&self.validation) {
            Ok(claims) => {
                request.extensions_mut().insert(claims);

                Ok(())
            }
            Err(err) => {
                debug!("Invalid JWT token: {}", err);

                Err(Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(ResBody::default())
                    .unwrap())
            }
        }
    }
}
