use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use tokio::sync::mpsc::Sender;

#[async_trait]
pub trait Logger: Send + Sync {
    async fn write_log_entry(&self, msg: String) -> Result<()>;
}

pub struct ResponseWriter {
    sender: Option<Sender<Bytes>>,
}

impl ResponseWriter {
    pub fn new(sender: Option<Sender<Bytes>>) -> Self {
        Self { sender }
    }
    pub async fn write_string(&self, value: String) -> Result<()> {
        info!("{value}");
        let buffer = encode_cbor(&value)?;
        if let Some(sender) = &self.sender {
            sender
                .send(buffer.into())
                .await
                .with_context(|| "writing response")?;
        }
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        if let Some(sender) = &self.sender {
            sender.is_closed()
        } else {
            false
        }
    }
}

#[async_trait]
impl Logger for ResponseWriter {
    async fn write_log_entry(&self, msg: String) -> Result<()> {
        self.write_string(msg).await
    }
}

pub struct LogSender {
    sender: Sender<(chrono::DateTime<chrono::Utc>, String)>,
}

impl LogSender {
    pub fn new(sender: Sender<(chrono::DateTime<chrono::Utc>, String)>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl Logger for LogSender {
    async fn write_log_entry(&self, msg: String) -> Result<()> {
        info!("{msg}");
        self.sender
            .send((chrono::Utc::now(), msg))
            .await
            .with_context(|| "LogSender::write_log_entry")
    }
}
