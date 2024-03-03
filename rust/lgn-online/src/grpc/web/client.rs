use std::task::{Context, Poll};

use http::{HeaderMap, HeaderValue, Method, Request, Response, Version};
use http_body::{combinators::UnsyncBoxBody, Body};
use tonic::codegen::{BoxFuture, StdError};
use tower::Service;
use tracing::{debug, warn};

use super::super::consts::{GRPC, GRPC_WEB, PROTOBUF};
use super::Error;

/// A gRPC-Web client wrapper that causes all outgoing `gRPC` requests to be
/// sent as HTTP 1.1 gRPC-Web requests.
#[derive(Clone)]
pub struct GrpcWebClient<C> {
    inner: C,
}

use super::super::buf::BoxBuf;
use super::GrpcWebResponse;

type RequestTransform<ReqBody> = fn(Request<ReqBody>) -> Result<Request<ReqBody>, Error>;
type ResponseTransform<F> = fn(F) -> BoxFuture<Response<UnsyncBoxBody<BoxBuf, Error>>, Error>;

impl<C> GrpcWebClient<C> {
    pub fn new(c: C) -> Self {
        Self { inner: c }
    }

    fn forward_request<ReqBody, ResBody>(
        &mut self,
        request: Request<ReqBody>,
    ) -> BoxFuture<Response<UnsyncBoxBody<BoxBuf, Error>>, Error>
    where
        C: Service<Request<ReqBody>, Response = Response<ResBody>>,
        C::Error: Into<StdError>,
        C::Future: Send + 'static,
        ResBody: http_body::Body + Send + 'static,
        <ResBody as http_body::Body>::Data: Send + 'static,
        <ResBody as http_body::Body>::Error: Into<StdError> + Send + 'static,
    {
        let resp = self.inner.call(request);

        Box::pin(async move {
            // This might look complex but we just wrap the response body data into a
            // `BoxBuf`, convert its error into an `Error`, and then wrap the
            // response body into a `UnsyncBoxBody` so that the end result
            // satisfies our response type.
            //
            // This allows us to keep any streaming/trailers support and pass them through
            // untouched.
            Ok(resp.await.map_err(Into::into)?.map(|body| {
                UnsyncBoxBody::new(
                    body.map_data(BoxBuf::new)
                        .map_err(Into::into)
                        .map_err(Error::Other),
                )
            }))
        })
    }

    fn get_transform_functions<ReqBody, ResBody>(
        proto: &str,
    ) -> Result<(RequestTransform<ReqBody>, ResponseTransform<C::Future>), Error>
    where
        C: Service<Request<ReqBody>, Response = Response<ResBody>>,
        C::Error: Into<StdError>,
        C::Future: Send + 'static,
        ResBody: http_body::Body + Send + 'static,
        <ResBody as http_body::Body>::Error: Into<StdError> + Send,
    {
        match proto {
            PROTOBUF => Ok((
                Self::coerce_request_into_protobuf,
                Self::coerce_response_from_protobuf,
            )),
            _ => Err(Error::UnsupportedGrpcProtocol(proto.to_string())),
        }
    }

    fn transform_request<ReqBody, ResBody>(
        &mut self,
        request: Request<ReqBody>,
        proto: &str,
    ) -> BoxFuture<Response<UnsyncBoxBody<BoxBuf, Error>>, Error>
    where
        C: Service<Request<ReqBody>, Response = Response<ResBody>>,
        C::Error: Into<StdError>,
        C::Future: Send + 'static,
        ResBody: http_body::Body + Send + 'static,
        <ResBody as http_body::Body>::Error: Into<StdError> + Send,
    {
        debug!("transforming gRPC call with protocol `{}`", proto);

        let (coerce_request, coerce_response) = match Self::get_transform_functions(proto) {
            Ok(f) => f,
            Err(e) => return Box::pin(async move { Err(e) }),
        };

        let resp = match coerce_request(request) {
            Ok(request) => self.inner.call(request),
            Err(err) => return Box::pin(async move { Err(err) }),
        };

        coerce_response(resp)
    }

    /// Transforms a `gRPC` HTTP 2 request into a `gRPC-Web` HTTP 1.1 request.
    ///
    /// We only support `application/grp-web+proto` as a content type for now.
    fn coerce_request_into_protobuf<ReqBody>(
        mut req: Request<ReqBody>,
    ) -> Result<Request<ReqBody>, Error> {
        let content_type = HeaderValue::from_str(&format!("{}+{}", GRPC_WEB, PROTOBUF))
            .map_err(Into::into)
            .map_err(Error::Other)?;

        // We must force HTTP 1.1 for `gRPC-web`.
        *req.version_mut() = http::Version::HTTP_11;

        req.headers_mut()
            .insert(http::header::CONTENT_TYPE, content_type.clone());
        req.headers_mut().insert(http::header::ACCEPT, content_type);

        // If `gRPC` clients were allowed to send trailers, this is where we would
        // extend the request and include them. Luckily for us, this is not
        // supported so we don't have to care.

        Ok(req)
    }

    /// Transforms a `gRPC`-web request into its HTTP 2 equivalent.
    ///
    /// This functions assumes that the request that caused the specified
    /// response was transformed with `coerce_request`.
    fn coerce_response_from_protobuf<ReqBody, ResBody>(
        resp: C::Future,
    ) -> BoxFuture<Response<UnsyncBoxBody<BoxBuf, Error>>, Error>
    where
        C: Service<Request<ReqBody>, Response = Response<ResBody>>,
        C::Error: Into<StdError>,
        C::Future: Send + 'static,
        ResBody: http_body::Body + Send + 'static,
        <ResBody as http_body::Body>::Error: Into<StdError> + Send,
    {
        Box::pin(async move {
            let content_type = HeaderValue::from_str(&format!("{}+{}", GRPC, PROTOBUF))
                .map_err(Into::into)
                .map_err(Error::Other)?;

            let (mut parts, body) = resp.await.map_err(Into::into)?.into_parts();
            parts.version = http::Version::HTTP_2;

            parts
                .headers
                .insert(http::header::CONTENT_TYPE, content_type);

            let body = GrpcWebResponse::new(body);

            let resp = Response::from_parts(parts, UnsyncBoxBody::new(body));

            Ok(resp)
        })
    }
}

impl<C, ReqBody, ResBody> Service<Request<ReqBody>> for GrpcWebClient<C>
where
    C: Service<Request<ReqBody>, Response = Response<ResBody>>,
    C::Error: Into<StdError>,
    C::Future: Send + 'static,
    ResBody: http_body::Body + Send + 'static,
    <ResBody as http_body::Body>::Data: Send,
    <ResBody as http_body::Body>::Error: Into<StdError> + Send,
{
    type Response = Response<UnsyncBoxBody<BoxBuf, Error>>;
    type Error = Error;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(Into::into)
            .map_err(Error::Other)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        use RequestKind::{Grpc, Other};

        match RequestKind::new(req.headers(), req.method(), req.version()) {
            Grpc {
                method: &Method::POST,
                proto,
            } => self.transform_request(req, &proto),
            Grpc { method, proto } => {
                warn!(
                    "refusing to handle gRPC call with invalid method `{}` and protocol `{}`",
                    method, proto
                );

                self.forward_request(req)
            }
            Other(..) => self.forward_request(req),
        }
    }
}

#[derive(Debug, PartialEq)]
enum RequestKind<'a> {
    // The request is considered a grpc request if its `content-type`
    // header is "application/grpc" and the HTTP method is HTTP 2.
    Grpc { method: &'a Method, proto: String },
    // All other requests, including `application/grpc`
    Other(Version),
}

impl<'a> RequestKind<'a> {
    fn new(headers: &'a HeaderMap, method: &'a Method, version: Version) -> Self {
        match headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
        {
            Some(content_type) => match content_type.strip_prefix(GRPC) {
                Some(proto) => {
                    let mut proto = proto.trim_start_matches('+');

                    if proto.is_empty() {
                        proto = PROTOBUF;
                    }

                    RequestKind::Grpc {
                        method,
                        proto: proto.to_string(),
                    }
                }
                None => RequestKind::Other(version),
            },
            None => RequestKind::Other(version),
        }
    }
}
