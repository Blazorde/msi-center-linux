//! Bridge between the GTK main loop and the D-Bus clients.
//!
//! zbus is built with its `tokio` feature for the whole workspace (the daemon and clipd run on tokio), so
//! the app runs its D-Bus work on a small tokio runtime (one worker thread) instead of the glib main
//! context. The GTK side awaits the [`tokio::task::JoinHandle`] returned by [`Backend::daemon`] /
//! [`Backend::clips`] (a plain future, pollable from `glib::spawn_future_local`) and receives pushed
//! [`Event`]s through an unbounded `tokio::sync::mpsc` channel, whose receiver is executor-agnostic.
//! Nothing blocks the GTK thread; widgets never leave it.
//!
//! Each service has a supervisor task: connect → read the status → forward events until the service's bus
//! name loses its owner → report it down → wait for a retry request or 10 s → reconnect.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::sync::{Notify, mpsc};
use zbus::Connection;

use crate::dbus::{self, Clips, Daemon, DaemonEvent};
use crate::errors::{AppError, Service};

/// Delay between automatic reconnection attempts.
pub const RETRY_DELAY: Duration = Duration::from_secs(10);

/// Pushed to the GTK side.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The daemon answered; `restore_factory` tells whether its API has `RestoreFactory`.
    DaemonUp {
        /// Optional method present.
        restore_factory: bool,
    },
    /// The daemon is unreachable.
    DaemonDown(AppError),
    /// Something from the daemon.
    Daemon(DaemonEvent),
    /// clipd answered.
    ClipdUp,
    /// clipd is unreachable.
    ClipdDown(AppError),
    /// The clip list changed.
    ClipsChanged,
}

/// Owns the tokio runtime and the current clients.
#[derive(Debug)]
pub struct Backend {
    rt: tokio::runtime::Runtime,
    daemon: Arc<Mutex<Option<Daemon>>>,
    clips: Arc<Mutex<Option<Clips>>>,
    retry_daemon: Arc<Notify>,
    retry_clipd: Arc<Notify>,
}

fn current<T: Clone>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().ok().and_then(|g| g.clone())
}

fn store<T>(slot: &Mutex<Option<T>>, v: Option<T>) {
    if let Ok(mut g) = slot.lock() {
        *g = v;
    }
}

async fn wait_retry(retry: &Notify) {
    tokio::select! {
        () = retry.notified() => {}
        () = tokio::time::sleep(RETRY_DELAY) => {}
    }
}

/// Resolves when `name` has no owner any more (the service stopped or crashed).
async fn owner_lost(conn: &Connection, name: &'static str) {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
        return std::future::pending().await;
    };
    let Ok(mut changes) = dbus.receive_name_owner_changed_with_args(&[(0, name)]).await else {
        return std::future::pending().await;
    };
    while let Some(sig) = changes.next().await {
        if sig.args().is_ok_and(|a| a.new_owner().is_none()) {
            return;
        }
    }
}

impl Backend {
    /// Starts the runtime (one worker thread).
    ///
    /// # Errors
    /// The runtime could not be built.
    pub fn new() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("msi-center-dbus")
            .enable_all()
            .build()?;
        Ok(Self {
            rt,
            daemon: Arc::default(),
            clips: Arc::default(),
            retry_daemon: Arc::default(),
            retry_clipd: Arc::default(),
        })
    }

    /// Starts both supervisors. Events go to `tx` until it is dropped.
    pub fn start(&self, tx: &mpsc::UnboundedSender<Event>) {
        self.rt.spawn(supervise_daemon(
            self.daemon.clone(),
            self.retry_daemon.clone(),
            tx.clone(),
        ));
        self.rt.spawn(supervise_clipd(
            self.clips.clone(),
            self.retry_clipd.clone(),
            tx.clone(),
        ));
    }

    /// Demo mode: serves both services in-process ([`crate::demo::service`]) and connects to them over
    /// private socket pairs. No bus is contacted; there is no reconnection.
    pub fn start_demo(&self, tx: &mpsc::UnboundedSender<Event>) {
        let (daemon, clips, tx) = (self.daemon.clone(), self.clips.clone(), tx.clone());
        self.rt.spawn(async move {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            let links = match crate::demo::service::connect(now_ms).await {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx.send(Event::DaemonDown(AppError::from_zbus(&e, Service::Daemon)));
                    let _ = tx.send(Event::ClipdDown(AppError::from_zbus(&e, Service::Clipd)));
                    return;
                }
            };
            let (d, c) = tokio::join!(
                run_daemon(&links.daemon, &daemon, &tx),
                run_clipd(&links.clipd, &clips, &tx)
            );
            if let Err(e) = d {
                let _ = tx.send(Event::DaemonDown(e));
            }
            if let Err(e) = c {
                let _ = tx.send(Event::ClipdDown(e));
            }
            drop(links);
        });
    }

    /// Asks a supervisor to reconnect now.
    pub fn retry(&self, service: Service) {
        match service {
            Service::Daemon => self.retry_daemon.notify_one(),
            Service::Clipd => self.retry_clipd.notify_one(),
        }
    }

    /// Runs `f` with the current daemon client on the D-Bus thread; await the result from GTK.
    ///
    /// # Errors
    /// `Unavailable` when not connected, or whatever `f` returns.
    pub fn daemon<T, F, Fut>(&self, f: F) -> impl Future<Output = dbus::Result<T>> + 'static
    where
        F: FnOnce(Daemon) -> Fut + Send + 'static,
        Fut: Future<Output = dbus::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let slot = self.daemon.clone();
        let handle = self.rt.spawn(async move {
            let d = current(&slot)
                .ok_or_else(|| AppError::Unavailable(Service::Daemon, "not connected".into()))?;
            f(d).await
        });
        async move { handle.await.map_err(|e| AppError::Internal(e.to_string()))? }
    }

    /// Runs `f` with the current clipd client on the D-Bus thread; await the result from GTK.
    ///
    /// # Errors
    /// `Unavailable` when not connected, or whatever `f` returns.
    pub fn clips<T, F, Fut>(&self, f: F) -> impl Future<Output = dbus::Result<T>> + 'static
    where
        F: FnOnce(Clips) -> Fut + Send + 'static,
        Fut: Future<Output = dbus::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let slot = self.clips.clone();
        let handle = self.rt.spawn(async move {
            let c = current(&slot)
                .ok_or_else(|| AppError::Unavailable(Service::Clipd, "not connected".into()))?;
            f(c).await
        });
        async move { handle.await.map_err(|e| AppError::Internal(e.to_string()))? }
    }
}

async fn supervise_daemon(
    slot: Arc<Mutex<Option<Daemon>>>,
    retry: Arc<Notify>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let mut conn: Option<Connection> = None;
    while !tx.is_closed() {
        if conn.is_none() {
            match Connection::system().await {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    let _ = tx.send(Event::DaemonDown(AppError::from_zbus(&e, Service::Daemon)));
                }
            }
        }
        if let Some(c) = conn.clone() {
            match run_daemon(&c, &slot, &tx).await {
                Ok(()) => {
                    let _ = tx.send(Event::DaemonDown(AppError::Unavailable(
                        Service::Daemon,
                        "stopped".into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(Event::DaemonDown(e));
                }
            }
            store(&slot, None);
        }
        wait_retry(&retry).await;
    }
}

/// One daemon session: returns when the daemon goes away.
async fn run_daemon(
    conn: &Connection,
    slot: &Mutex<Option<Daemon>>,
    tx: &mpsc::UnboundedSender<Event>,
) -> dbus::Result<()> {
    let d = Daemon::new(conn).await?;
    let mut events = d.events().await?;
    let status = d.status().await?;
    let restore_factory = d.has_method(dbus::RESTORE_FACTORY).await;
    store(slot, Some(d));
    let _ = tx.send(Event::DaemonUp { restore_factory });
    let _ = tx.send(Event::Daemon(DaemonEvent::Status(status)));
    let forward = async {
        while let Some(ev) = events.next().await {
            if tx.send(Event::Daemon(ev)).is_err() {
                break;
            }
        }
    };
    tokio::select! {
        () = forward => {}
        () = owner_lost(conn, dbus::DAEMON_NAME) => {}
    }
    Ok(())
}

async fn supervise_clipd(
    slot: Arc<Mutex<Option<Clips>>>,
    retry: Arc<Notify>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let mut conn: Option<Connection> = None;
    while !tx.is_closed() {
        if conn.is_none() {
            match Connection::session().await {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    let _ = tx.send(Event::ClipdDown(AppError::from_zbus(&e, Service::Clipd)));
                }
            }
        }
        if let Some(c) = conn.clone() {
            match run_clipd(&c, &slot, &tx).await {
                Ok(()) => {
                    let _ = tx.send(Event::ClipdDown(AppError::Unavailable(
                        Service::Clipd,
                        "stopped".into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(Event::ClipdDown(e));
                }
            }
            store(&slot, None);
        }
        wait_retry(&retry).await;
    }
}

async fn run_clipd(
    conn: &Connection,
    slot: &Mutex<Option<Clips>>,
    tx: &mpsc::UnboundedSender<Event>,
) -> dbus::Result<()> {
    let c = Clips::new(conn).await?;
    let mut changes = c.changes().await?;
    // Activates clipd and checks it answers.
    c.tags().await?;
    store(slot, Some(c));
    let _ = tx.send(Event::ClipdUp);
    let forward = async {
        while changes.next().await.is_some() {
            if tx.send(Event::ClipsChanged).is_err() {
                break;
            }
        }
    };
    tokio::select! {
        () = forward => {}
        () = owner_lost(conn, dbus::CLIPD_NAME) => {}
    }
    Ok(())
}
