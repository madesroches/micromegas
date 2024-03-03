use std::task::Poll;
use thiserror::Error;
use tonic::codegen::BoxFuture;
use tonic::codegen::StdError;
use tower::{Layer, Service};
use tracing::prelude::*;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Access Denied")]
    AccessDenied, //opaque by design, the reason why will be logged
    #[error(transparent)]
    Other(#[from] StdError),
}

pub async fn validate_auth<T>(
    user_info_url: &str,
    request: &http::Request<T>,
) -> Result<(), AuthError> {
    match request
        .headers()
        .get("Authorization")
        .map(http::header::HeaderValue::to_str)
    {
        None => {
            error!("Auth: no token in request");
            Err(AuthError::AccessDenied)
        }
        Some(Err(_)) => {
            error!("Auth: error parsing token");
            Err(AuthError::AccessDenied)
        }
        Some(Ok(auth)) => {
            let resp = reqwest::Client::new()
                .get(user_info_url)
                .header("Authorization", auth)
                .send()
                .await;
            if let Err(e) = resp {
                error!("Error validating credentials: {}", e);
                return Err(AuthError::AccessDenied);
            }
            let content = resp.unwrap().text().await;
            if let Err(e) = content {
                error!("Error reading user info response: {}", e);
                return Err(AuthError::AccessDenied);
            }
            let text_content = content.unwrap();
            let userinfo = serde_json::from_str::<serde_json::Value>(&text_content);
            if let Err(e) = userinfo {
                error!("Error parsing user info response: {} {}", e, text_content);
                return Err(AuthError::AccessDenied);
            }
            let email = &userinfo.unwrap()["email"];
            if !email.is_string() {
                error!("Email not found in user info response: {}", &text_content);
                return Err(AuthError::AccessDenied);
            }
            info!("authenticated user: {}", &text_content);
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthLayer {
    pub user_info_url: String,
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthServiceWrapper<S>;

    fn layer(&self, service: S) -> Self::Service {
        AuthServiceWrapper {
            inner: service,
            user_info_url: self.user_info_url.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthServiceWrapper<S> {
    inner: S,
    user_info_url: String,
}

impl<S> Service<http::Request<tonic::transport::Body>> for AuthServiceWrapper<S>
where
    S: Service<http::Request<tonic::transport::Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<tonic::codegen::StdError> + Send + 'static,
{
    type Response = S::Response;
    type Error = AuthError;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(Into::into)
            .map_err(AuthError::Other)
    }

    fn call(&mut self, req: http::Request<tonic::transport::Body>) -> Self::Future {
        let clone = self.inner.clone();
        let user_info_url = self.user_info_url.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            //todo: move health check in its own layer
            if req.method() == http::method::Method::OPTIONS
                || req.uri() == "/health.Health/Check"
                || req.uri() == "/health"
            {
                return inner
                    .call(req)
                    .await
                    .map_err(Into::into)
                    .map_err(AuthError::Other);
            }

            match validate_auth(&user_info_url, &req).await {
                Ok(_) => inner
                    .call(req)
                    .await
                    .map_err(Into::into)
                    .map_err(AuthError::Other),
                Err(e) => Err(e),
            }
        })
    }
}
