use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::{Request, Response};
use pin_project::pin_project;
use tower::Service;

use super::{Error, HyperClient, Result};
use lgn_auth::{Authenticator, BoxedAuthenticator};

/// A `gRPC` client wrapper that adds authentication.
#[derive(Debug)]
pub struct AuthenticatedClient<C = HyperClient, A = BoxedAuthenticator> {
    client: C,
    authenticator: Option<Arc<A>>,
    scopes: Vec<String>,
    /// Typically the identity provider
    extra_params: Option<HashMap<String, String>>,
}

impl<C, A> Clone for AuthenticatedClient<C, A>
where
    C: Clone,
{
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            authenticator: self.authenticator.clone(),
            scopes: self.scopes.clone(),
            extra_params: self.extra_params.clone(),
        }
    }
}

impl<C, A> AuthenticatedClient<C, A>
where
    A: Authenticator,
{
    pub fn new(client: C, authenticator: Option<A>, scopes: &[String]) -> Self {
        Self {
            client,
            authenticator: authenticator.map(Arc::new),
            scopes: scopes.to_vec(),
            extra_params: None,
        }
    }

    pub fn set_extra_params(&mut self, extra_params: Option<HashMap<String, String>>) -> &mut Self {
        self.extra_params = extra_params;

        self
    }
}

impl<C, A, ReqBody, ResBody> Service<Request<ReqBody>> for AuthenticatedClient<C, A>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>> + Send + Clone,
    C::Error: Into<Error>,
    A: Authenticator + Send + Sync + 'static,
{
    type Response = C::Response;
    type Error = Error;
    type Future = ResponseFuture<C, ReqBody, ResBody>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.client.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        match &self.authenticator {
            Some(authenticator) => {
                let authenticator = authenticator.clone();
                let scopes = self.scopes.clone();
                let extra_params = self.extra_params.clone();

                Self::Future::new_with_authentication(
                    async move {
                        Ok(authenticator
                            .login(&scopes, &extra_params)
                            .await?
                            .access_token)
                    },
                    self.client.clone(),
                    req,
                )
            }
            None => Self::Future::new_without_authentication(self.client.clone(), req),
        }
    }
}

#[pin_project(project=ResponseFutureProj)]
pub enum ResponseFuture<C, ReqBody, ResBody>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    Authenticating(
        #[pin] Pin<Box<dyn Future<Output = Result<String>> + Send + 'static>>,
        C,
        Option<Request<ReqBody>>,
    ),
    Authenticated(#[pin] C::Future),
}

impl<C, ReqBody, ResBody> ResponseFuture<C, ReqBody, ResBody>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    fn new_with_authentication(
        f: impl Future<Output = Result<String>> + Send + 'static,
        client: C,
        req: Request<ReqBody>,
    ) -> Self {
        Self::Authenticating(Box::pin(f), client, Some(req))
    }

    fn new_without_authentication(mut client: C, req: Request<ReqBody>) -> Self {
        Self::Authenticated(client.call(req))
    }
}

impl<C, ReqBody, ResBody> Future for ResponseFuture<C, ReqBody, ResBody>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
    C::Error: Into<Error>,
{
    type Output = Result<Response<ResBody>, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let state = match self.as_mut().project() {
                ResponseFutureProj::Authenticating(f, client, req) => match f.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Ready(Ok(access_token)) => {
                        let mut req = std::mem::take(req).unwrap();

                        req.headers_mut().insert(
                            "Authorization",
                            format!("Bearer {}", access_token).parse().unwrap(),
                        );

                        ResponseFuture::Authenticated(client.call(req))
                    }
                },
                ResponseFutureProj::Authenticated(f) => {
                    return match f.poll(cx) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(Err(err)) => Poll::Ready(Err(err.into())),
                        Poll::Ready(Ok(res)) => Poll::Ready(Ok(res)),
                    }
                }
            };

            self.set(state);
        }
    }
}
