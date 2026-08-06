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
///
/// The channel carries `Result<(time, msg), String>`: an `Err` item ends the stream with a
/// genuine `RecordBatchStream` error (propagating through `execute_stream`/`collect` as a query
/// execution failure) instead of being folded into one more `(time, msg)` log row -- see
/// `tasks/blocks_view_ordered_merges_plan.md`'s Design §3.
pub struct AsyncLogStream {
    schema: SchemaRef,
    rx: mpsc::Receiver<Result<(chrono::DateTime<chrono::Utc>, String), String>>,
}

impl AsyncLogStream {
    pub fn new(
        schema: SchemaRef,
        rx: mpsc::Receiver<Result<(chrono::DateTime<chrono::Utc>, String), String>>,
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

        // An Err item ends the stream as a query error. Ok items received in the same poll batch
        // ahead of the Err are dropped -- acceptable, they are transient progress lines on a
        // query that is failing anyway.
        let mut times = PrimitiveBuilder::<TimestampNanosecondType>::with_capacity(messages.len());
        let mut msgs = StringBuilder::new();
        for msg in messages {
            match msg {
                Ok((time, text)) => {
                    times.append_value(time.timestamp_nanos_opt().unwrap_or_default());
                    msgs.append_value(text);
                }
                Err(err_msg) => {
                    return Poll::Ready(Some(Err(DataFusionError::Execution(err_msg))));
                }
            }
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
