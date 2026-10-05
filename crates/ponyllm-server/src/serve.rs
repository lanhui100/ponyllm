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
use std::future::IntoFuture;
use tokio::sync::watch;

/// Drain budget: after the shutdown signal, the server waits at most this
/// long for in-flight requests before forcing shutdown. Must stay below the
/// Deployment `terminationGracePeriodSeconds` minus `preStop` sleep.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Serve forever until `shutdown_rx` flips to `true`, then gracefully drain
/// in-flight connections (SSE streams included) up to `drain_timeout`.
///
/// The drain budget starts ONLY after the shutdown signal: with no signal the
/// server runs indefinitely (a timeout around the whole serve future would
/// deterministically kill every long-lived process — production 503, 2026-09-29).
///
/// Note on "force close": dropping the `axum::serve` future stops the accept
/// loop but in-flight connection tasks spawned by hyper survive until the
/// PROCESS exits (the CLI's outer timeout then returns and the tokio runtime
/// shuts down, which does the real truncation — matching kubelet's kill after
/// `terminationGracePeriodSeconds`). The in-process drain deadline therefore
/// only guarantees the server GIVES UP waiting; client-side EOF/RST is
/// process-exit-dependent (HA ADR: long upstreams are truncated, clients
/// retry).
pub async fn serve_with_shutdown(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
) -> std::io::Result<()> {
    let signal_rx = shutdown_rx.clone();
    let shutdown_future = async move {
        wait_shutdown_flag(shutdown_rx).await;
    };
    // `with_connect_info` (Phase-2 F3): registers the TCP peer address in the
    // request extensions so `auth::resolve_client_ip` can fall back to the
    // real socket peer when forwarding headers are absent/untrusted.
    let mut serve = Box::pin(
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_future)
        .into_future(),
    );
    // Phase 1: normal operation, deliberately NO deadline.
    tokio::select! {
        result = &mut serve => {
            // The server ended on its own (listener error etc.) with no
            // shutdown signal in play.
            return result;
        }
        _ = wait_shutdown_flag(signal_rx) => {
            // Signal arrived: fall through to the bounded drain below.
        }
    }
    // Phase 2: drain budget ticks only from the signal onward.
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

/// Resolve once `rx` flips to `true`; never resolve if the sender is dropped
/// (the server task then outlives any signal wiring and keeps serving).
async fn wait_shutdown_flag(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // Sender dropped → nothing will ever flip the flag; keep
            // serving (the server task outlives any signal wiring).
            futures_util::future::pending::<()>().await;
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
    async fn serve_with_shutdown_survives_past_drain_deadline_without_signal() {
        // Regression (production 503, 2026-09-29): the drain budget must not
        // tick while the server is healthy — with no signal the server stays up
        // indefinitely, even far beyond `drain_timeout`.
        let (tx, rx) = watch::channel(false);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/health", get(|| async { "ok" }));
        let handle = tokio::spawn(async move {
            serve_with_shutdown(listener, app, rx, Duration::from_millis(100)).await
        });
        // Live well past the drain budget with no signal: still serving.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !handle.is_finished(),
            "server must not exit without a shutdown signal"
        );
        let body = reqwest::get(format!("http://{addr}/health"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
        // A real signal still drains promptly.
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