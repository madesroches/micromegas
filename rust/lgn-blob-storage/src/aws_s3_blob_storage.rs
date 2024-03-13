use async_trait::async_trait;
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use pin_project::pin_project;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::str::FromStr;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::{fmt::Display, pin::Pin};
use tokio::io::AsyncWrite;
use tokio_stream::Stream;
use tokio_util::io::StreamReader;

use super::{BlobStats, BoxedAsyncRead, BoxedAsyncWrite, Error, Result, StreamingBlobStorage};

pub struct AwsS3BlobStorage {
    url: AwsS3Url,
    client: aws_sdk_s3::Client,
}

impl AwsS3BlobStorage {
    #[span_fn]
    pub async fn new(url: AwsS3Url) -> Self {
        let config = aws_config::load_from_env().await;
        let client = aws_sdk_s3::Client::new(&config);

        Self { url, client }
    }

    fn blob_key(&self, name: &str) -> String {
        format!("{}/{}", self.url.root, name)
    }

    #[span_fn]
    async fn get_object_size(&self, key: &str) -> Result<Option<u64>> {
        let req = self
            .client
            .get_object()
            .bucket(&self.url.bucket_name)
            .key(key);

        match req.send().await {
            Ok(output) => Ok(Some(output.content_length() as u64)),
            Err(aws_sdk_s3::types::SdkError::ServiceError { err, raw: _ }) => {
                if let aws_sdk_s3::error::GetObjectErrorKind::NoSuchKey(_) = err.kind {
                    Ok(None)
                } else {
                    Err(Error::forward_with_context(
                        err,
                        format!("could not fetch AWS S3 object: {}", key),
                    ))
                }
            }
            Err(err) => Err(Error::forward_with_context(
                err,
                format!("unexpected SDK error while fetching AWS object: {}", key),
            )),
        }
    }
}

#[pin_project]
#[derive(Debug)]
struct ByteStreamReader(#[pin] aws_sdk_s3::types::ByteStream);

impl Stream for ByteStreamReader {
    type Item = std::result::Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project()
            .0
            .poll_next(cx)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

struct ByteStreamWriter {
    client: aws_sdk_s3::Client,
    bucket_name: String,
    key: String,
    state: Mutex<ByteStreamWriterState>,
}

type ByteStreamWriterBoxedFuture = Box<
    dyn Future<
            Output = std::result::Result<
                aws_sdk_s3::output::PutObjectOutput,
                aws_sdk_s3::types::SdkError<aws_sdk_s3::error::PutObjectError>,
            >,
        > + Send
        + 'static,
>;

enum ByteStreamWriterState {
    Writing(Vec<u8>),
    Uploading(Pin<ByteStreamWriterBoxedFuture>),
}

impl ByteStreamWriter {
    #[span_fn]
    fn new(client: aws_sdk_s3::Client, bucket_name: String, key: String) -> Self {
        Self {
            client,
            bucket_name,
            key,
            state: Mutex::new(ByteStreamWriterState::Writing(Vec::new())),
        }
    }

    #[span_fn]
    fn poll_write_impl(&self, buf: &[u8]) -> Poll<std::result::Result<usize, std::io::Error>> {
        match &mut *self.state.lock().unwrap() {
            ByteStreamWriterState::Writing(buffer) => {
                buffer.extend_from_slice(buf);

                Poll::Ready(Ok(buf.len()))
            }
            ByteStreamWriterState::Uploading(_) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "cannot write to an uploading stream",
            ))),
        }
    }

    #[span_fn]
    fn poll_flush_impl(&self) -> Poll<std::result::Result<(), std::io::Error>> {
        match &*self.state.lock().unwrap() {
            ByteStreamWriterState::Writing(_) => Poll::Ready(Ok(())),
            ByteStreamWriterState::Uploading(_) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "cannot flush an uploading stream",
            ))),
        }
    }

    #[span_fn]
    fn poll_shutdown_impl(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        let mut state = self.state.lock().unwrap();

        let fut = match &mut *state {
            ByteStreamWriterState::Writing(buffer) => {
                let body = aws_sdk_s3::types::ByteStream::from(std::mem::take(buffer));

                let fut = self
                    .client
                    .put_object()
                    .bucket(&self.bucket_name)
                    .key(&self.key)
                    .body(body)
                    .send();

                *state = ByteStreamWriterState::Uploading(Box::pin(fut));

                if let ByteStreamWriterState::Uploading(fut) = &mut *state {
                    fut
                } else {
                    unreachable!()
                }
            }
            ByteStreamWriterState::Uploading(fut) => fut,
        };

        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => {
                Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, err)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for ByteStreamWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::result::Result<usize, std::io::Error>> {
        self.poll_write_impl(buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        self.poll_flush_impl()
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        self.poll_shutdown_impl(cx)
    }
}

#[async_trait]
impl StreamingBlobStorage for AwsS3BlobStorage {
    async fn get_blob_info(&self, name: &str) -> super::Result<Option<BlobStats>> {
        async_span_scope!("AwsS3BlobStorage::get_blob_info");
        let key = self.blob_key(name);

        let size = self.get_object_size(&key).await?;

        Ok(size.map(|size| BlobStats { size }))
    }

    async fn get_blob_reader(&self, name: &str) -> Result<BoxedAsyncRead> {
        async_span_scope!("AwsS3BlobStorage::get_blob_reader");
        let key = self.blob_key(name);

        let req = self
            .client
            .get_object()
            .bucket(&self.url.bucket_name)
            .key(&key);

        let object = req.send().await.map_err(|e| {
            Error::forward_with_context(e, format!("could not download blob from AWS S3: {}", key))
        })?;

        let bytestream = ByteStreamReader(object.body);
        let stream = StreamReader::new(bytestream);

        Ok(Box::pin(stream))
    }

    async fn get_blob_writer(&self, name: &str) -> Result<Option<BoxedAsyncWrite>> {
        async_span_scope!("AwsS3BlobStorage::get_blob_writer");
        let key = self.blob_key(name);

        if self.blob_exists(&key).await? {
            return Ok(None);
        }

        let writer = ByteStreamWriter::new(self.client.clone(), self.url.bucket_name.clone(), key);

        Ok(Some(Box::pin(writer)))
    }

    async fn delete_blob(&self, name: &str) -> Result<()> {
        async_span_scope!("AwsS3BlobStorage::delete_blob");
        let key = self.blob_key(name);

        match self
            .client
            .delete_object()
            .bucket(&self.url.bucket_name)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => Err(Error::forward_with_context(
                err,
                format!("unexpected SDK error while deleting AWS object: {}", key),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AwsS3Url {
    pub bucket_name: String,
    pub root: String,
}

impl Display for AwsS3Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s3://{}/{}", self.bucket_name, self.root)
    }
}

impl FromStr for AwsS3Url {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self, Self::Err> {
        s.parse::<Url>()?.try_into()
    }
}

impl TryFrom<Url> for AwsS3Url {
    type Error = anyhow::Error;

    fn try_from(value: Url) -> anyhow::Result<Self, Self::Error> {
        Ok(Self {
            bucket_name: value
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("invalid S3 URL: missing bucket name"))?
                .to_owned(),
            root: value.path().trim_start_matches('/').to_string(),
        })
    }
}

impl Serialize for AwsS3Url {
    fn serialize<S>(&self, serializer: S) -> anyhow::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AwsS3Url {
    fn deserialize<D>(deserializer: D) -> anyhow::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
