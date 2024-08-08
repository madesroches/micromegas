use anyhow::{Context, Result};
use bytes::Bytes;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use tokio::sync::mpsc::Sender;

pub struct ResponseWriter {
    sender: Option<Sender<Bytes>>,
}

impl ResponseWriter {
    pub fn new(sender: Option<Sender<Bytes>>) -> Self {
        Self { sender }
    }
    pub async fn write_string(&self, value: &str) -> Result<()> {
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
