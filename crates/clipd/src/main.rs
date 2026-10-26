//! `clipd`: user service exposing `org.blazorde.Clipd1` on the session bus.
//!
//! Log level: `CLIPD_LOG` (`EnvFilter` syntax, default `info`).

#![forbid(unsafe_code)]

use anyhow::Context as _;
use clip_core::{Library, Limits};
use clipd::{BUS_NAME, Clipd, OBJECT_PATH, Paths, SqliteStore};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_env("CLIPD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Under systemd, journald already timestamps lines and does not render colours.
    let journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(!journald);
    if journald {
        builder.without_time().init();
    } else {
        builder.init();
    }
}

/// Waits for SIGTERM (systemd stop, logout) or SIGINT (Ctrl+C).
async fn shutdown_signal() -> anyhow::Result<&'static str> {
    let mut term = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut int = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;
    Ok(tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let paths = Paths::from_env(|k| std::env::var_os(k)).context("resolving storage locations")?;
    let store = SqliteStore::open(&paths).context("opening clip store")?;
    let mut lib = Library::new(store, Limits::default());
    let evicted = lib.enforce_retention().context("applying retention")?;
    if !evicted.is_empty() {
        tracing::info!(evicted = evicted.len(), "evicted clips over the limit at startup");
    }

    let service = Clipd::new(lib);
    let shared = service.library();
    let conn = zbus::connection::Builder::session()
        .name(BUS_NAME)
        .serve_at(OBJECT_PATH, service)
        .build()
        .await
        .context("connecting to the session bus")?;
    tracing::info!(name = BUS_NAME, path = OBJECT_PATH, "serving");

    let sig = shutdown_signal().await?;
    tracing::info!(signal = sig, "shutting down");
    // Stop accepting calls, then wait for any in-flight storage call to finish its transaction.
    conn.graceful_shutdown().await;
    tokio::task::spawn_blocking(move || drop(shared.lock()))
        .await
        .context("waiting for in-flight calls")?;
    tracing::info!("stopped");
    Ok(())
}
