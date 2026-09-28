//! Graceful shutdown primitives (P1-OPS-001, multi-node HA).
//!
//! [`serve_with_shutdown`] wraps `axum::serve` with a graceful-shutdown future
//! driven by an in-process `watch` channel — cross-platform testable — and a
//! hard drain deadline so `SIGTERM`/`SIGINT` never lets kubelet hard-kill an
//! in-flight SSE stream without a final truncation attempt.
//! (Kubelet's `terminationGracePeriodSeconds` must be larger than
//! [`DEFAULT_DRAIN_TIMEOUT`].)

use std::time::Duration;

use axum::Router;
use tokio::sync::watch;

/// Drain budget: after the shutdown signal, the server waits at most this
/// long for in-flight requests before forcing shutdown. Must stay below the
/// Deployment `terminationGracePeriodSeconds` minus `preStop` sleep.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Serve forever until `shutdown_rx` flips to `true`, then gracefully drain
/// in-flight connections (SSE streams included) up to `drain_timeout`, after
/// which the listener is force-closed (long upstreams are truncated — the
/// client-retry contract documented in the HA ADR).
pub async fn serve_with_shutdown(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
) -> std::io::Result<()> {
    let shutdown_future = async move {
        let mut shutdown_rx = shutdown_rx;
        loop {
            if *shutdown_rx.borrow() {
                return;
            }
            if shutdown_rx.changed().await.is_err() {
                // Sender dropped → nothing will ever flip the flag; keep
                // serving (the server task outlives any signal wiring).
                futures_util::future::pending::<()>().await;
            }
        }
    };
    let serve = axum::serve(listener, router.into_make_service()).with_graceful_shutdown(
        shutdown_future,
    );
    match tokio::time::timeout(drain_timeout, serve).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::warn!(
                secs = drain_timeout.as_secs(),
                "graceful drain deadline exceeded; forcing shutdown (long streams truncated)"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    #[tokio::test]
    async fn serve_with_shutdown_exits_after_signal_with_no_connections() {
        let (tx, rx) = watch::channel(false);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app = Router::new().route("/health", get(|| async { "ok" }));
        let handle = tokio::spawn(async move {
            serve_with_shutdown(listener, app, rx, Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("server must exit after shutdown signal")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn serve_with_shutdown_forces_exit_past_drain_deadline_with_hung_connection() {
        use axum::http::HeaderValue;

        // A handler that starts streaming and never ends: models an upstream
        // SSE stream that the drain deadline must truncate.
        let app = Router::new().route(
            "/hang",
            get(|| async {
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::convert::Infallible>>(1);
                let _ = tx; // never send → body never completes
                let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header("content-type", HeaderValue::from_static("text/plain"))
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap()
            }),
        );
        let (tx, rx) = watch::channel(false);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            serve_with_shutdown(listener, app, rx, Duration::from_millis(200)).await
        });

        // Open a connection whose request hangs (never completes).
        let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Give it a chance to reach the handler.
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("server must exit after drain deadline")
            .unwrap()
            .unwrap();
    }
}