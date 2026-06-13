use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use micromegas_tracing::prelude::*;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower::Service;

/// Completes when SIGTERM is received. On non-unix targets, never completes
/// (preserves current behavior; production deploys are Linux/ECS).
#[cfg(unix)]
pub async fn wait_for_sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    sigterm.recv().await;
}

#[cfg(not(unix))]
pub async fn wait_for_sigterm() {
    std::future::pending::<()>().await
}

/// Fans a shutdown future out to N consumers via a watch channel.
///
/// `subscribe()` returns a future that completes once the original shutdown
/// future has fired — usable as the drain trigger for axum/tonic and as the
/// deadline arm. Subscribers created after the signal has fired complete
/// immediately.
pub struct ShutdownFanout {
    tx: watch::Sender<bool>,
}

impl ShutdownFanout {
    pub fn new<F>(shutdown: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (tx, _rx) = watch::channel(false);
        let tx2 = tx.clone();
        tokio::spawn(async move {
            shutdown.await;
            let _ = tx2.send(true);
        });
        Self { tx }
    }

    /// Returns a future that completes once the shutdown signal has fired.
    pub fn subscribe(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = self.tx.subscribe();
        async move {
            let _ = rx.wait_for(|v| *v).await;
        }
    }
}

/// Serves an Axum application, draining in-flight requests when `shutdown` fires.
///
/// Logs when the signal is received, when drain completes cleanly, or when the
/// grace period expires with work still in flight.
pub async fn serve_axum_with_graceful_shutdown<M, S, F>(
    listener: TcpListener,
    make_service: M,
    shutdown: F,
    grace: Duration,
) -> Result<()>
where
    M: for<'a> Service<
            axum::serve::IncomingStream<'a, TcpListener>,
            Error = Infallible,
            Response = S,
        > + Send
        + 'static,
    for<'a> <M as Service<axum::serve::IncomingStream<'a, TcpListener>>>::Future: Send,
    S: Service<axum::extract::Request, Response = axum::response::Response, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
    F: Future<Output = ()> + Send + 'static,
{
    use std::future::IntoFuture;

    let grace_secs = grace.as_secs();
    let fanout = ShutdownFanout::new(shutdown);

    let drain = fanout.subscribe();
    let axum_shutdown = async move {
        drain.await;
        info!("draining, grace={grace_secs}s");
    };

    let serve_future = axum::serve(listener, make_service)
        .with_graceful_shutdown(axum_shutdown)
        .into_future();

    let deadline = {
        let d = fanout.subscribe();
        async move {
            d.await;
            tokio::time::sleep(grace).await;
        }
    };

    tokio::select! {
        res = serve_future => {
            info!("drain completed");
            res.map_err(anyhow::Error::from)
        }
        _ = deadline => {
            warn!("grace period of {grace_secs}s elapsed with work still in flight");
            Ok(())
        }
    }
}
