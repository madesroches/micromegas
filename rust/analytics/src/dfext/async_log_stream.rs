use datafusion::{
    arrow::{
        array::{PrimitiveBuilder, RecordBatch, StringBuilder},
        datatypes::{SchemaRef, TimestampNanosecondType},
    },
    common::Result,
    error::DataFusionError,
    execution::RecordBatchStream,
};
use futures::Stream;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc;

/// A stream of log messages that can be converted into a `RecordBatchStream`.
pub struct AsyncLogStream {
    schema: SchemaRef,
    rx: mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)>,
}

impl AsyncLogStream {
    pub fn new(
        schema: SchemaRef,
        rx: mpsc::Receiver<(chrono::DateTime<chrono::Utc>, String)>,
    ) -> Self {
        Self { schema, rx }
    }
}

impl Stream for AsyncLogStream {
    type Item = Result<RecordBatch>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut messages = vec![];
        let limit = self.rx.max_capacity();
        if self
            .rx
            .poll_recv_many(cx, &mut messages, limit)
            .is_pending()
        {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if messages.is_empty() {
            if self.rx.is_closed() {
                // channel closed, aborting
                return Poll::Ready(None);
            }
            // not sure this can happen
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        let mut times = PrimitiveBuilder::<TimestampNanosecondType>::with_capacity(messages.len());
        let mut msgs = StringBuilder::new();
        for msg in messages {
            times.append_value(msg.0.timestamp_nanos_opt().unwrap_or_default());
            msgs.append_value(msg.1);
        }

        let rb_res = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(times.finish().with_timezone_utc()),
                Arc::new(msgs.finish()),
            ],
        )
        .map_err(|e| DataFusionError::ArrowError(e.into(), None));
        Poll::Ready(Some(rb_res))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.rx.len(), Some(self.rx.len()))
    }
}

impl RecordBatchStream for AsyncLogStream {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}
