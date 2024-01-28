use std::{
    pin::Pin,
    task::{Context, Poll},
};

use http_body::Body;
use pin_project::pin_project;
use tonic::codegen::StdError;

use super::super::buf::BoxBuf;
use super::{Error, GrpcWebBodyParser, Result};

#[pin_project]
pub(super) struct GrpcWebResponse<T: Body> {
    #[pin]
    inner: T,
    body_parser: GrpcWebBodyParser,
}

impl<T: Body> GrpcWebResponse<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            body_parser: GrpcWebBodyParser::default(),
        }
    }
}

/// A response to a `gRPC-Web` request that un-webize the response and
/// translates it back into a classic `gRPC` response.
impl<T> Body for GrpcWebResponse<T>
where
    T: Body,
    T::Error: Into<StdError>,
{
    type Data = BoxBuf;
    type Error = Error;

    fn poll_data(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Self::Data>>> {
        let this = self.project();
        match this.inner.poll_data(cx) {
            Poll::Ready(Some(Ok(data))) => {
                this.body_parser.put(data);

                this.body_parser.poll_data()
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Error::Other(e.into())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<Option<http::HeaderMap>>> {
        let this = self.project();

        match this.body_parser.poll_trailers() {
            Poll::Ready(x) => Poll::Ready(x),
            Poll::Pending => match this.inner.poll_data(cx) {
                Poll::Ready(Some(Ok(data))) => {
                    this.body_parser.put(data);

                    this.body_parser.poll_trailers()
                }
                Poll::Ready(Some(Err(e))) => Poll::Ready(Err(Error::Other(e.into()))),
                Poll::Ready(None) => this.body_parser.set_poll_complete(),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
