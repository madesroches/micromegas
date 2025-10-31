//! Connection info utilities for capturing client socket addresses in Tonic services.

use futures::Stream;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tonic::transport::server::Connected;

/// A wrapper around a TCP stream that captures and provides the remote socket address.
/// This is used with Tonic's `serve_with_incoming` to make client IPs available.
pub struct ConnectedStream<T> {
    inner: T,
    remote_addr: SocketAddr,
}

impl<T> ConnectedStream<T> {
    pub fn new(inner: T, remote_addr: SocketAddr) -> Self {
        Self { inner, remote_addr }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for ConnectedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for ConnectedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Implement Connected trait to provide the remote address to Tonic
impl<T> Connected for ConnectedStream<T> {
    type ConnectInfo = SocketAddr;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.remote_addr
    }
}

/// An incoming stream adapter that wraps TCP connections with ConnectedStream
pub struct ConnectedIncoming {
    inner: Pin<Box<dyn Stream<Item = Result<tokio::net::TcpStream, io::Error>> + Send>>,
}

impl ConnectedIncoming {
    pub fn from_std_listener(listener: std::net::TcpListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        Ok(Self::new(listener))
    }

    pub fn new(listener: tokio::net::TcpListener) -> Self {
        let stream = async_stream::stream! {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => yield Ok(stream),
                    Err(e) => yield Err(e),
                }
            }
        };

        Self {
            inner: Box::pin(stream),
        }
    }
}

impl Stream for ConnectedIncoming {
    type Item = Result<ConnectedStream<tokio::net::TcpStream>, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(stream))) => {
                let remote_addr = match stream.peer_addr() {
                    Ok(addr) => addr,
                    Err(e) => return Poll::Ready(Some(Err(e))),
                };
                Poll::Ready(Some(Ok(ConnectedStream::new(stream, remote_addr))))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
