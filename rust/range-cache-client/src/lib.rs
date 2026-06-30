use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use bytes::{Buf, Bytes};
use futures::stream::{self, BoxStream};
use micromegas_tracing::prelude::*;
use object_store::{
    Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult, path::Path,
};
use reqwest::Client;
use serde_json::json;
use std::ops::Range;
use std::sync::Arc;

#[derive(Debug)]
pub struct CacheClientStore {
    http: Client,
    cache_base_url: String,
    api_key: Option<String>,
    direct: Arc<dyn ObjectStore>,
}

impl CacheClientStore {
    pub fn new(
        cache_base_url: String,
        api_key: Option<String>,
        direct: Arc<dyn ObjectStore>,
    ) -> Self {
        let http = Client::builder().build().expect("building reqwest client");
        Self {
            http,
            cache_base_url,
            api_key,
            direct,
        }
    }

    fn obj_url(&self, location: &Path) -> String {
        format!(
            "{}/obj/{}",
            self.cache_base_url.trim_end_matches('/'),
            location.as_ref()
        )
    }

    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.bearer_auth(key)
        } else {
            req
        }
    }

    async fn get_range_bytes(&self, location: &Path, range: Range<u64>) -> Result<Bytes> {
        let url = self.obj_url(location);
        let range_header = format!("bytes={}-{}", range.start, range.end.saturating_sub(1));
        let resp = self
            .add_auth(self.http.get(&url))
            .header("Range", range_header)
            .send()
            .await
            .with_context(|| "sending GET to cache")?;
        if !resp.status().is_success() {
            return Err(anyhow!("cache GET {url} status {}", resp.status()));
        }
        resp.bytes().await.with_context(|| "reading GET response")
    }

    async fn get_full_bytes(&self, location: &Path) -> Result<Bytes> {
        let url = self.obj_url(location);
        let resp = self
            .add_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| "sending GET to cache")?;
        if !resp.status().is_success() {
            return Err(anyhow!("cache GET {url} status {}", resp.status()));
        }
        resp.bytes().await.with_context(|| "reading GET response")
    }

    async fn head_size(&self, location: &Path) -> Result<u64> {
        let url = self.obj_url(location);
        let resp = self
            .add_auth(self.http.head(&url))
            .send()
            .await
            .with_context(|| "sending HEAD to cache")?;
        if !resp.status().is_success() {
            return Err(anyhow!("cache HEAD {url} status {}", resp.status()));
        }
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| anyhow!("missing Content-Length in HEAD response"))
    }
}

impl std::fmt::Display for CacheClientStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CacheClientStore({})", self.cache_base_url)
    }
}

fn bytes_to_get_result(location: &Path, data: Bytes, range: Option<Range<u64>>) -> GetResult {
    let size = data.len() as u64;
    let actual_range = range.unwrap_or(0..size);
    let meta = ObjectMeta {
        location: location.clone(),
        last_modified: chrono::Utc::now(),
        size,
        e_tag: None,
        version: None,
    };
    let payload = GetResultPayload::Stream(Box::pin(stream::once(async move { Ok(data) })));
    GetResult {
        payload,
        meta,
        range: actual_range,
        attributes: Attributes::default(),
    }
}

#[async_trait]
impl ObjectStore for CacheClientStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.direct.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.direct.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        use object_store::GetRange;

        let result: Result<GetResult> = match &options.range {
            None => self
                .get_full_bytes(location)
                .await
                .map(|data| bytes_to_get_result(location, data, None)),
            Some(GetRange::Bounded(r)) => self
                .get_range_bytes(location, r.start..r.end)
                .await
                .map(|data| bytes_to_get_result(location, data, Some(r.start..r.end))),
            Some(GetRange::Offset(offset)) => match self.head_size(location).await {
                Ok(size) => {
                    let start = *offset;
                    self.get_range_bytes(location, start..size)
                        .await
                        .map(|data| {
                            let len = data.len() as u64;
                            bytes_to_get_result(location, data, Some(start..start + len))
                        })
                }
                Err(e) => Err(e),
            },
            Some(GetRange::Suffix(suffix)) => match self.head_size(location).await {
                Ok(size) => {
                    let start = size.saturating_sub(*suffix);
                    self.get_range_bytes(location, start..size)
                        .await
                        .map(|data| {
                            let len = data.len() as u64;
                            bytes_to_get_result(location, data, Some(start..start + len))
                        })
                }
                Err(e) => Err(e),
            },
        };

        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                warn!("cache miss for {location}, falling back to direct: {e}");
                self.direct.get_opts(location, options).await
            }
        }
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        let url = format!(
            "{}/obj/{}/ranges",
            self.cache_base_url.trim_end_matches('/'),
            location.as_ref()
        );
        let ranges_json: Vec<[u64; 2]> = ranges.iter().map(|r| [r.start, r.end]).collect();
        let body = json!({ "ranges": ranges_json }).to_string();

        let resp = match self
            .add_auth(
                self.http
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(body),
            )
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                warn!("cache ranges {url} status {}, falling back", r.status());
                return self.direct.get_ranges(location, ranges).await;
            }
            Err(e) => {
                warn!("cache ranges request failed: {e}, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
        };

        let mut data = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                warn!("reading ranges response failed: {e}, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
        };

        let mut results = Vec::with_capacity(ranges.len());
        for _ in 0..ranges.len() {
            if data.remaining() < 8 {
                warn!("truncated ranges response, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
            let len = data.get_u64_le() as usize;
            if data.remaining() < len {
                warn!("truncated ranges response body, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
            results.push(data.copy_to_bytes(len));
        }

        Ok(results)
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.direct.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.direct.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.direct.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.direct.copy_opts(from, to, options).await
    }
}
