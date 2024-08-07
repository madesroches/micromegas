use anyhow::{Context, Result};
use bytes::Bytes;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use tokio::sync::mpsc::Sender;

pub struct ResponseWriter {
    sender: Sender<Bytes>,
}

impl ResponseWriter {
    pub fn new(sender: Sender<Bytes>) -> Self {
        Self { sender }
    }
    pub async fn write_string(&self, value: &str) -> Result<()> {
        info!("{value}");
        let buffer = encode_cbor(&value)?;
        self.sender
            .send(buffer.into())
            .await
            .with_context(|| "writing response")?;
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}
