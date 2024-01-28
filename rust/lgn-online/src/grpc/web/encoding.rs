use std::str::FromStr;
use std::task::Poll;

use bytes::{Buf, BufMut, BytesMut};
use http::{header::HeaderName, HeaderMap, HeaderValue};
use tracing::debug;

use super::super::buf::BoxBuf;
use super::{Error, Result};

pub(super) struct GrpcWebBodyParser {
    buf: BytesMut,
    body_bytes_remaining: Option<usize>,
    trailers_bytes_remaining: Option<usize>,
}

impl Default for GrpcWebBodyParser {
    fn default() -> Self {
        Self {
            buf: BytesMut::with_capacity(1024),
            body_bytes_remaining: None,
            trailers_bytes_remaining: None,
        }
    }
}

impl GrpcWebBodyParser {
    /// Append some data the parser.
    pub fn put(&mut self, b: impl Buf) {
        debug!(
            "GrpcWebBodyParser::put received {} byte(s): {:x?}",
            b.remaining(),
            b.chunk()
        );

        self.buf.put(b);
    }

    /// Return any chunk of data that can be passed on to the next layer
    /// already.
    pub fn poll_data(&mut self) -> Poll<Option<Result<BoxBuf>>> {
        match self.body_bytes_remaining {
            // We don't have any clue how many bytes we're going to get.
            // We need to keep those bytes aside until we get enough.
            None => {
                // We still don't have enough. Let's wait for more.
                if self.buf.remaining() < 5 {
                    debug!("GrpcWebBodyParser::poll_data does not have enough header bytes yet to know the body length ({} byte(s) out of 5)", self.buf.remaining());

                    return Poll::Pending;
                }

                // We have enough bytes to know how much more we need.
                let body_header = self.buf.copy_to_bytes(5);
                let body_length = body_header.slice(1..5).get_u32() as usize;

                Poll::Ready(if self.buf.remaining() >= body_length {
                    // We have enough bytes to return the body completely.
                    //
                    // Set `body_bytes_remaining` to `Some(0)` to reflect that we already sent back
                    // the body.
                    self.body_bytes_remaining = Some(0);

                    debug!("GrpcWebBodyParser::poll_data can return the complete header and body ({} byte(s) with {} extra trailers byte(s))", body_length, self.buf.remaining() - body_length);

                    Some(Ok(BoxBuf::new(
                        body_header.chain(self.buf.copy_to_bytes(body_length)),
                    )))
                } else if self.buf.remaining() > 0 {
                    self.body_bytes_remaining = Some(body_length - self.buf.remaining());

                    debug!("GrpcWebBodyParser::poll_data can return the complete header and part of the body ({} byte(s) out of {} body byte(s))", self.buf.remaining(), body_length);

                    Some(Ok(BoxBuf::new(
                        body_header.chain(self.buf.copy_to_bytes(self.buf.remaining())),
                    )))
                } else {
                    self.body_bytes_remaining = Some(body_length);

                    debug!("GrpcWebBodyParser::poll_data can return the complete header but not body bytes");

                    Some(Ok(BoxBuf::new(body_header)))
                })
            }
            Some(0) => {
                debug!(
                    "GrpcWebBodyParser::poll_data does not have any body bytes to return anymore"
                );

                Poll::Ready(None)
            }
            Some(body_bytes_remaining) => {
                Poll::Ready(if self.buf.remaining() >= body_bytes_remaining {
                    // We have enough bytes to return the body completely.
                    //
                    // Set `body_bytes_remaining` to `Some(0)` to reflect that we already sent back
                    // the body.
                    self.body_bytes_remaining = Some(0);

                    debug!(
                        "GrpcWebBodyParser::poll_data can return the complete remaining body ({} byte(s) with {} extra trailers byte(s))",
                        body_bytes_remaining,
                        self.buf.remaining() - body_bytes_remaining
                    );

                    Some(Ok(BoxBuf::new(
                        self.buf.copy_to_bytes(body_bytes_remaining),
                    )))
                } else {
                    self.body_bytes_remaining = Some(body_bytes_remaining - self.buf.remaining());

                    debug!(
                        "GrpcWebBodyParser::poll_data can return part of the remaining body ({} byte(s))",
                        body_bytes_remaining,
                    );

                    Some(Ok(BoxBuf::new(
                        self.buf.copy_to_bytes(self.buf.remaining()),
                    )))
                })
            }
        }
    }

    fn poll_trailers_impl(
        &mut self,
        trailers_bytes_required: usize,
    ) -> Poll<Result<Option<HeaderMap>>> {
        if self.buf.remaining() >= trailers_bytes_required {
            // We have enough bytes to parse the trailers.
            let buf = self.buf.copy_to_bytes(trailers_bytes_required).to_vec();

            let trailers = String::from_utf8(buf)
                .map_err(|err| {
                    Error::InvalidGrpcWebBody(format!("invalid trailers UTF-8 string: {}", err))
                })?
                .trim_end()
                .split("\r\n")
                .map(parse_trailer)
                .collect::<Result<_>>()?;

            self.trailers_bytes_remaining = Some(0);

            Poll::Ready(Ok(Some(trailers)))
        } else {
            Poll::Pending
        }
    }

    pub fn poll_trailers(&mut self) -> Poll<Result<Option<HeaderMap>>> {
        match self.body_bytes_remaining {
            Some(0) => match self.trailers_bytes_remaining {
                None => {
                    if self.buf.remaining() >= 5 {
                        let mut buf = self.buf.copy_to_bytes(5);

                        match buf.get_u8() {
                            0b10000000 => {
                                let trailers_bytes_remaining = buf.get_u32() as usize;
                                self.trailers_bytes_remaining = Some(trailers_bytes_remaining);

                                return self.poll_trailers_impl(trailers_bytes_remaining);
                            }
                            0b10000001 => {
                                return Poll::Ready(Err(Error::InvalidGrpcWebBody(
                                    "trailers frame is compressed and we do not support it yet"
                                        .to_string(),
                                )))
                            }
                            x => {
                                return Poll::Ready(Err(Error::InvalidGrpcWebBody(format!(
                                    "trailers compression byte has an unexpected value: {}",
                                    x
                                ))))
                            }
                        }
                    }

                    debug!(
                        "GrpcWebBodyParser::poll_trailers does not have enough trailer bytes yet to know the trailers length ({} byte(s) out of 5)",
                        self.buf.remaining()
                    );

                    Poll::Pending
                }
                Some(0) => Poll::Ready(Ok(None)),
                Some(trailers_bytes_required) => self.poll_trailers_impl(trailers_bytes_required),
            },
            None | Some(_) => Poll::Pending,
        }
    }

    pub fn set_poll_complete(&mut self) -> Poll<Result<Option<HeaderMap>>> {
        match self.body_bytes_remaining {
            None => {
                if self.buf.remaining() > 0 {
                    Poll::Ready(Err(Error::InvalidGrpcWebBody(format!(
                        "incomplete body: {} byte(s) remaining in buffer",
                        self.buf.remaining()
                    ))))
                } else {
                    debug!("GrpcWebBodyParser::set_poll_complete has completed successfullly with no body");
                    Poll::Ready(Ok(None))
                }
            }
            Some(0) => match self.trailers_bytes_remaining {
                None => {
                    if self.buf.remaining() > 0 {
                        Poll::Ready(Err(Error::InvalidGrpcWebBody(format!(
                            "incomplete trailers: {} byte(s) remaining in buffer",
                            self.buf.remaining()
                        ))))
                    } else {
                        debug!("GrpcWebBodyParser::set_poll_complete has completed successfullly with no trailers");
                        Poll::Ready(Ok(None))
                    }
                }
                Some(0) => Poll::Ready(Ok(None)),
                Some(missing_bytes_len) => Poll::Ready(Err(Error::InvalidGrpcWebBody(format!(
                    "missing {} trailer byte(s)",
                    missing_bytes_len
                )))),
            },
            Some(missing_bytes_len) => Poll::Ready(Err(Error::InvalidGrpcWebBody(format!(
                "missing {} body byte(s)",
                missing_bytes_len
            )))),
        }
    }
}

/// Parse a HTTP trailer header, returning a `HeaderName` and `HeaderValue`
/// pair.
///
/// # Arguments
///
/// * `s` - The trailer header to parse, in the format `name:value` with any
///   number of whitespaces
/// around the different parts.
fn parse_trailer(s: &str) -> Result<(HeaderName, HeaderValue)> {
    let mut parts = s.trim().splitn(2, ':');

    Ok((
        HeaderName::from_str(
            parts
                .next()
                .ok_or_else(|| {
                    Error::InvalidGrpcWebBody("invalid trailer with missing name".to_string())
                })?
                .trim_end(),
        )
        .map_err(|err| Error::InvalidGrpcWebBody(format!("invalid trailer name: {}", err)))?,
        HeaderValue::from_str(
            parts
                .next()
                .ok_or_else(|| {
                    Error::InvalidGrpcWebBody(format!("invalid trailer `{}` with missing value", s))
                })?
                .trim_start(),
        )
        .map_err(|err| Error::InvalidGrpcWebBody(format!("invalid trailer value: {}", err)))?,
    ))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn test_grpc_web_body_parser_complete_buffer() {
        let body = Bytes::from(vec![
            0, 0, 0, 0, 7, 10, 5, 104, 101, 108, 108, 111, 128, 0, 0, 0, 15, 103, 114, 112, 99, 45,
            115, 116, 97, 116, 117, 115, 58, 48, 13, 10,
        ]);

        let mut parser = GrpcWebBodyParser::default();

        parser.put(body.clone());

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 12);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[..12]);
            }
            _ => panic!("expected a complete buffer"),
        }
        match parser.poll_data() {
            Poll::Ready(None) => {}
            _ => panic!("expected no more reads"),
        }
        match parser.poll_trailers() {
            Poll::Ready(Ok(Some(trailers))) => {
                assert_eq!(
                    trailers,
                    vec![(
                        HeaderName::from_static("grpc-status"),
                        HeaderValue::from_static("0")
                    )]
                    .into_iter()
                    .collect()
                );
            }
            _ => panic!("expected trailers"),
        }
        match parser.poll_trailers() {
            Poll::Ready(Ok(None)) => {}
            _ => panic!("expected no more reads for trailers"),
        }
        match parser.set_poll_complete() {
            Poll::Ready(Ok(None)) => {}
            _ => panic!("expected poll to be complete successfully"),
        }
    }

    #[test]
    fn test_grpc_web_body_parser_complete_body_header_first() {
        let body = Bytes::from(vec![
            0, 0, 0, 0, 7, 10, 5, 104, 101, 108, 108, 111, 128, 0, 0, 0, 15, 103, 114, 112, 99, 45,
            115, 116, 97, 116, 117, 115, 58, 48, 13, 10,
        ]);

        let mut parser = GrpcWebBodyParser::default();

        parser.put(body.slice(..5));

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 5);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[..5]);
            }
            _ => panic!("expected a complete buffer"),
        }

        parser.put(body.slice(5..));

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 7);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[5..12]);
            }
            _ => panic!("expected a complete buffer"),
        }
        match parser.poll_data() {
            Poll::Ready(None) => {}
            _ => panic!("expected no more reads"),
        }
        match parser.poll_trailers() {
            Poll::Ready(Ok(Some(trailers))) => {
                assert_eq!(
                    trailers,
                    vec![(
                        HeaderName::from_static("grpc-status"),
                        HeaderValue::from_static("0")
                    )]
                    .into_iter()
                    .collect()
                );
            }
            _ => panic!("expected trailers"),
        }
        match parser.poll_trailers() {
            Poll::Ready(Ok(None)) => {}
            _ => panic!("expected no more reads for trailers"),
        }
        match parser.set_poll_complete() {
            Poll::Ready(Ok(None)) => {}
            _ => panic!("expected poll to be complete successfully"),
        }
    }

    #[test]
    fn test_grpc_web_body_parser_complete_body_chunked() {
        let body = Bytes::from(vec![
            0, 0, 0, 0, 7, 10, 5, 104, 101, 108, 108, 111, 128, 0, 0, 0, 15, 103, 114, 112, 99, 45,
            115, 116, 97, 116, 117, 115, 58, 48, 13, 10,
        ]);

        let mut parser = GrpcWebBodyParser::default();

        parser.put(body.slice(..7));

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 7);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[..7]);
            }
            _ => panic!("expected a complete buffer"),
        }

        parser.put(body.slice(7..9));

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 2);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[7..9]);
            }
            _ => panic!("expected a complete buffer"),
        }

        parser.put(body.slice(9..14));

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 3);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[9..12]);
            }
            _ => panic!("expected a complete buffer"),
        }
        match parser.poll_data() {
            Poll::Ready(None) => {}
            _ => panic!("expected no more reads"),
        }
        match parser.poll_trailers() {
            Poll::Pending => {}
            Poll::Ready(x) => panic!("expected no trailers yet: got {:?}", x),
        }

        parser.put(body.slice(14..20));

        match parser.poll_trailers() {
            Poll::Pending => {}
            Poll::Ready(x) => panic!("expected no trailers yet: got {:?}", x),
        }

        parser.put(body.slice(20..));

        match parser.poll_trailers() {
            Poll::Ready(Ok(Some(trailers))) => {
                assert_eq!(
                    trailers,
                    vec![(
                        HeaderName::from_static("grpc-status"),
                        HeaderValue::from_static("0")
                    )]
                    .into_iter()
                    .collect()
                );
            }
            _ => panic!("expected trailers"),
        }
        match parser.poll_trailers() {
            Poll::Ready(Ok(None)) => {}
            _ => panic!("expected no more reads for trailers"),
        }
    }

    #[test]
    fn test_grpc_web_body_parser_body_cut_off() {
        let body = Bytes::from(vec![0, 0, 0, 0, 2, 10]);

        let mut parser = GrpcWebBodyParser::default();

        parser.put(body.clone());

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 6);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[..6]);
            }
            _ => panic!("expected a complete buffer"),
        }

        if let Poll::Ready(Ok(_)) = parser.set_poll_complete() {
            panic!("expected poll to be incomplete")
        }
    }

    #[test]
    fn test_grpc_web_body_parser_trailers_cut_off() {
        let body = Bytes::from(vec![0, 0, 0, 0, 1, 10, 0]);

        let mut parser = GrpcWebBodyParser::default();

        parser.put(body.clone());

        match parser.poll_data() {
            Poll::Ready(Some(Ok(mut data))) => {
                assert_eq!(data.remaining(), 6);
                assert_eq!(data.copy_to_bytes(data.remaining())[..], body[..6]);
            }
            _ => panic!("expected a complete buffer"),
        }

        if let Poll::Ready(Ok(_)) = parser.set_poll_complete() {
            panic!("expected poll to be incomplete")
        }
    }

    #[test]
    fn test_parse_trailer_ok() {
        let s = "key:value";
        let (key, value) = parse_trailer(s).unwrap();
        assert_eq!(key, "key");
        assert_eq!(value, "value");
    }

    #[test]
    fn test_parse_trailer_whitespaces_ok() {
        let s = "  key : \tvalue  \r\n";
        let (key, value) = parse_trailer(s).unwrap();
        assert_eq!(key, "key");
        assert_eq!(value, "value");
    }

    #[test]
    #[should_panic]
    fn test_parse_trailer_no_value() {
        let s = "  key  ";
        parse_trailer(s).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_parse_trailer_empty() {
        let s = "";
        parse_trailer(s).unwrap();
    }
}
