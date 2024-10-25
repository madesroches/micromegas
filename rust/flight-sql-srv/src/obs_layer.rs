use std::task::{Context, Poll};

use tower::Service;

#[derive(Clone)]
pub struct LogService<S> {
    pub service: S,
}

impl<S, B> Service<http::request::Request<B>> for LogService<S>
where
    S: Service<http::request::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, request: http::request::Request<B>) -> Self::Future {
        // Log the request
        println!("uri={}", request.uri());

        self.service.call(request)
    }
}
