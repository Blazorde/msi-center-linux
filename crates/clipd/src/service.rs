//! D-Bus interface `org.blazorde.Clipd1` (see `docs/architecture/dbus-api.md`).
//!
//! Every call runs the (blocking) SQLite work on Tokio's blocking pool under one mutex, so calls are
//! serialised and the async executor is never blocked. Mutating calls emit `Changed`.
//!
//! Clip content is never logged: only ids, MIME types and sizes.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clip_core::{AddKind, ClipId, ClipMeta, Library, LibraryError, Limits, Query, Scope, Timestamp};
use zbus::object_server::SignalEmitter;
use zbus::{DBusError, interface};

use crate::store::{SqliteStore, StoreError};

pub const BUS_NAME: &str = "org.blazorde.Clipd1";
pub const OBJECT_PATH: &str = "/org/blazorde/Clipd";

/// D-Bus errors, named `org.blazorde.Clipd1.Error.<Variant>`.
#[derive(Debug, DBusError)]
#[zbus(prefix = "org.blazorde.Clipd1.Error")]
pub enum ClipdError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// No clip with this id.
    NotFound(String),
    /// Content refused: empty, too large, unsupported MIME type, invalid UTF-8 text.
    Rejected(String),
    /// Invalid argument: bad tag, bad scope.
    InvalidArgument(String),
    /// Storage failure (details are in the service log).
    Internal(String),
}

impl From<LibraryError<StoreError>> for ClipdError {
    fn from(e: LibraryError<StoreError>) -> Self {
        match e {
            LibraryError::Rejected(r) => Self::Rejected(r.to_string()),
            LibraryError::Tag(t) => Self::InvalidArgument(t.to_string()),
            LibraryError::Query(q) => Self::InvalidArgument(q.to_string()),
            LibraryError::NotFound(id) => Self::NotFound(format!("no clip with id {id}")),
            LibraryError::Store(s) => {
                tracing::error!(error = %s, "storage failure");
                Self::Internal("storage failure".into())
            }
        }
    }
}

/// One `List` entry: `(t id, s mime, s preview, t size, x created_ms, x last_used_ms, b pinned, as tags)`.
pub type ClipEntry = (u64, String, String, u64, i64, i64, bool, Vec<String>);

fn entry(meta: ClipMeta) -> ClipEntry {
    (
        meta.id.0,
        meta.mime.as_str().to_owned(),
        meta.preview,
        meta.size,
        meta.created.0,
        meta.last_used.0,
        meta.pinned,
        meta.tags.to_strings(),
    )
}

/// Wall-clock time in milliseconds since the Unix epoch.
#[must_use]
pub fn system_clock() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp(ms)
}

pub type SharedLibrary = Arc<Mutex<Library<SqliteStore>>>;

#[derive(Debug)]
pub struct Clipd {
    lib: SharedLibrary,
    limits: Limits,
    clock: fn() -> Timestamp,
    /// Last timestamp handed out, to keep them strictly increasing (see [`Clipd::now`]).
    last: AtomicI64,
}

impl Clipd {
    #[must_use]
    pub fn new(lib: Library<SqliteStore>) -> Self {
        Self::with_clock(lib, system_clock)
    }

    /// Uses `clock` instead of the system clock (tests).
    #[must_use]
    pub fn with_clock(lib: Library<SqliteStore>, clock: fn() -> Timestamp) -> Self {
        let limits = *lib.limits();
        Self {
            lib: Arc::new(Mutex::new(lib)),
            limits,
            clock,
            last: AtomicI64::new(i64::MIN),
        }
    }

    /// Current time, strictly greater than any previously returned one, so that a re-copy within the
    /// same millisecond (or after a backwards clock step) still moves the clip to the top.
    fn now(&self) -> Timestamp {
        let wall = (self.clock)().0;
        let mut last = self.last.load(Ordering::SeqCst);
        loop {
            let next = wall.max(last.saturating_add(1));
            match self
                .last
                .compare_exchange_weak(last, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Timestamp(next),
                Err(actual) => last = actual,
            }
        }
    }

    /// Shared handle on the library, e.g. to wait for in-flight calls at shutdown.
    #[must_use]
    pub fn library(&self) -> SharedLibrary {
        Arc::clone(&self.lib)
    }

    async fn run<T, F>(&self, f: F) -> Result<T, ClipdError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Library<SqliteStore>) -> Result<T, ClipdError> + Send + 'static,
    {
        let lib = Arc::clone(&self.lib);
        tokio::task::spawn_blocking(move || {
            let mut guard = lib
                .lock()
                .map_err(|_| ClipdError::Internal("state lock poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "blocking task failed");
            ClipdError::Internal("task failed".into())
        })?
    }

    async fn notify(emitter: &SignalEmitter<'_>) {
        if let Err(e) = Self::changed(emitter).await {
            tracing::warn!(error = %e, "failed to emit Changed");
        }
    }
}

#[interface(name = "org.blazorde.Clipd1")]
impl Clipd {
    /// Stores clipboard content and returns its id. Identical content (same MIME type and bytes) is not
    /// duplicated: the existing clip is bumped to the top and its id returned.
    async fn add(
        &self,
        mime: String,
        data: Vec<u8>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<u64, ClipdError> {
        let now = self.now();
        let size = data.len();
        let outcome = self.run(move |lib| Ok(lib.add(&mime, data, now)?)).await?;
        tracing::debug!(
            id = %outcome.id,
            size,
            bumped = outcome.kind == AddKind::Bumped,
            evicted = outcome.evicted.len(),
            "add"
        );
        Self::notify(&emitter).await;
        Ok(outcome.id.0)
    }

    /// Lists clips, most recently used first. `query`: case-insensitive substring of the text (empty =
    /// no filter); `tags`: clips must carry all of them; `scope`: `all`/`""`, `pinned`, `unpinned`;
    /// `limit` 0 = 50, max 1000. Returns the total number of matches and the requested page.
    #[zbus(out_args("total", "clips"))]
    async fn list(
        &self,
        query: String,
        tags: Vec<String>,
        scope: String,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ClipEntry>), ClipdError> {
        let q = Scope::parse(&scope)
            .and_then(|scope| Query::new(&query, &tags, scope, offset, limit))
            .map_err(|e| ClipdError::InvalidArgument(e.to_string()))?;
        let page = self.run(move |lib| Ok(lib.list(&q)?)).await?;
        Ok((page.total, page.clips.into_iter().map(entry).collect()))
    }

    /// Full content of a clip.
    #[zbus(out_args("mime", "data"))]
    async fn get(&self, id: u64) -> Result<(String, Vec<u8>), ClipdError> {
        let (mime, data) = self.run(move |lib| Ok(lib.get(ClipId(id))?)).await?;
        Ok((mime.as_str().to_owned(), data))
    }

    /// Pins a clip: kept forever, never evicted. Idempotent.
    async fn pin(
        &self,
        id: u64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ClipdError> {
        self.run(move |lib| Ok(lib.pin(ClipId(id))?)).await?;
        tracing::debug!(id, "pin");
        Self::notify(&emitter).await;
        Ok(())
    }

    /// Unpins a clip: it goes back to the session history (erased at logout). Idempotent.
    async fn unpin(
        &self,
        id: u64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ClipdError> {
        let evicted = self.run(move |lib| Ok(lib.unpin(ClipId(id))?)).await?;
        tracing::debug!(id, evicted = evicted.len(), "unpin");
        Self::notify(&emitter).await;
        Ok(())
    }

    /// Deletes clips (pinned or not). Unknown ids are ignored. Returns how many were deleted.
    async fn delete(
        &self,
        ids: Vec<u64>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<u32, ClipdError> {
        let ids: Vec<ClipId> = ids.into_iter().map(ClipId).collect();
        let n = self.run(move |lib| Ok(lib.delete(&ids)?)).await?;
        tracing::debug!(deleted = n, "delete");
        if n > 0 {
            Self::notify(&emitter).await;
        }
        Ok(n)
    }

    /// Deletes every unpinned clip. Returns how many were deleted.
    async fn clear_unpinned(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<u32, ClipdError> {
        let n = self.run(|lib| Ok(lib.clear_unpinned()?)).await?;
        tracing::debug!(deleted = n, "clear unpinned");
        if n > 0 {
            Self::notify(&emitter).await;
        }
        Ok(n)
    }

    /// Replaces the tags of a clip; returns them normalised (trimmed, lowercased, deduplicated, sorted).
    async fn set_tags(
        &self,
        id: u64,
        tags: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<Vec<String>, ClipdError> {
        let set = self.run(move |lib| Ok(lib.set_tags(ClipId(id), &tags)?)).await?;
        tracing::debug!(id, count = set.len(), "set tags");
        Self::notify(&emitter).await;
        Ok(set.to_strings())
    }

    /// Every tag in use with its number of clips, sorted by tag.
    async fn tags(&self) -> Result<Vec<(String, u32)>, ClipdError> {
        let tags = self.run(|lib| Ok(lib.tags()?)).await?;
        Ok(tags
            .into_iter()
            .map(|(t, n)| (t.as_str().to_owned(), n))
            .collect())
    }

    /// Emitted after any change to the stored clips.
    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// Maximum number of unpinned clips kept per session.
    #[zbus(property)]
    fn max_unpinned(&self) -> u32 {
        u32::try_from(self.limits.max_unpinned).unwrap_or(u32::MAX)
    }

    /// Maximum size of one clip, in bytes.
    #[zbus(property)]
    fn max_clip_bytes(&self) -> u64 {
        self.limits.max_clip_bytes as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    fn frozen() -> Timestamp {
        Timestamp(1000)
    }

    #[test]
    fn timestamps_strictly_increase_even_with_a_frozen_clock() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("run"), dir.path().join("data"));
        let lib = Library::new(SqliteStore::open(&paths).unwrap(), Limits::default());
        let svc = Clipd::with_clock(lib, frozen);
        assert_eq!(svc.now(), Timestamp(1000));
        assert_eq!(svc.now(), Timestamp(1001));
        assert_eq!(svc.now(), Timestamp(1002));
    }
}
