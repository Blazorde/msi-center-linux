//! Versioned schema migrations, tracked with `PRAGMA user_version`.
//!
//! The session DB and the pinned DB share the same schema, so a clip moves between them row for row.
//! Append new migrations at the end; never edit a released one.
//!
//! Column order matters: `data` (up to 10 MiB) is last so that reading metadata never walks the blob's
//! overflow pages.

use rusqlite::Connection;

use crate::store::StoreError;

const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE clips (
        id           INTEGER PRIMARY KEY,
        hash         BLOB    NOT NULL UNIQUE,
        mime         TEXT    NOT NULL,
        size         INTEGER NOT NULL,
        created_ms   INTEGER NOT NULL,
        last_used_ms INTEGER NOT NULL,
        preview      TEXT    NOT NULL,
        search       TEXT,
        data         BLOB    NOT NULL
    );
    CREATE INDEX clips_by_use ON clips (last_used_ms, id);
    CREATE TABLE clip_tags (
        clip_id INTEGER NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
        tag     TEXT    NOT NULL,
        PRIMARY KEY (clip_id, tag)
    ) WITHOUT ROWID;
    CREATE INDEX clip_tags_by_tag ON clip_tags (tag);
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value INTEGER NOT NULL
    ) WITHOUT ROWID;",
];

/// Latest schema version.
#[must_use]
pub fn latest_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

/// Brings `conn`'s main database up to [`latest_version`]. Each step runs in its own transaction.
///
/// # Errors
///
/// [`StoreError::SchemaTooNew`] if the database was written by a newer `clipd`, or an SQLite error.
pub fn migrate(conn: &mut Connection) -> Result<u32, StoreError> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let latest = latest_version();
    if current > latest {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    for (version, sql) in (1..)
        .zip(MIGRATIONS)
        .skip(usize::try_from(current).unwrap_or(usize::MAX))
    {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::info!(version, "applied schema migration");
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_db_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        let v: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, latest_version());
    }

    #[test]
    fn refuses_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", latest_version() + 1)
            .unwrap();
        assert!(matches!(migrate(&mut conn), Err(StoreError::SchemaTooNew { .. })));
    }
}
