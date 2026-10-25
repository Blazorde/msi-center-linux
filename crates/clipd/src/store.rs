//! SQLite implementation of [`ClipStore`] (ADR-0005).
//!
//! One connection opens the session DB (tmpfs) as `main` and attaches the pinned DB (disk) as `pinned`.
//! Both use the same schema ([`crate::migrations`]). Pinning moves a row (and its tags) from `main` to
//! `pinned` in a single transaction. Both databases use the rollback journal (`journal_mode=DELETE`, not
//! WAL) so that SQLite commits multi-database transactions atomically (super-journal); a crash can
//! therefore never leave a clip in both tiers or in neither. As a belt-and-braces measure, opening the
//! store also drops session rows whose content already exists in the pinned DB.
//!
//! Files are created `0600` and directories `0700`. `secure_delete` is on, so deleted clip content is
//! overwritten in the DB file rather than left in free pages.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clip_core::{
    Candidate, ClipId, ClipMeta, ClipStore, ContentHash, Mime, Page, Query, Tag, TagSet, Timestamp,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, params_from_iter};
use thiserror::Error;

use crate::migrations;
use crate::paths::Paths;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("database schema v{found} is newer than this clipd supports (v{supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("clip id space exhausted")]
    IdsExhausted,
}

/// Tier schema names in the connection.
const SESSION: &str = "main";
const PINNED: &str = "pinned";

const META_COLUMNS: &str = "id, hash, mime, size, created_ms, last_used_ms, preview, pinned";

#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
    next_id: u64,
}

fn io_err<'a>(action: &'static str, path: &'a Path) -> impl FnOnce(std::io::Error) -> StoreError + 'a {
    move |source| StoreError::Io {
        action,
        path: path.to_owned(),
        source,
    }
}

/// Creates `dir` (and missing parents) with mode 0700 and forces 0700 on `dir` itself.
fn prepare_dir(dir: &Path) -> Result<(), StoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io_err("create directory", dir))?;
    fs::set_permissions(dir, Permissions::from_mode(0o700)).map_err(io_err("chmod", dir))
}

/// Creates `file` with mode 0600 if missing and forces 0600. SQLite gives its journal the same mode.
fn prepare_file(file: &Path) -> Result<(), StoreError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(file)
        .map_err(io_err("create", file))?;
    fs::set_permissions(file, Permissions::from_mode(0o600)).map_err(io_err("chmod", file))
}

fn configure(conn: &Connection, schema: &str) -> Result<(), StoreError> {
    // journal_mode returns a row, so it is queried rather than updated.
    let _mode: String = conn.query_row(&format!("PRAGMA {schema}.journal_mode = DELETE"), [], |r| {
        r.get(0)
    })?;
    conn.execute_batch(&format!(
        "PRAGMA {schema}.secure_delete = ON; PRAGMA {schema}.synchronous = FULL;"
    ))?;
    Ok(())
}

fn open_and_migrate(path: &Path) -> Result<(), StoreError> {
    prepare_file(path)?;
    let mut conn = Connection::open(path)?;
    configure(&conn, SESSION)?;
    migrations::migrate(&mut conn)?;
    Ok(())
}

fn to_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::IdsExhausted)
}

fn id_param(id: ClipId) -> i64 {
    // Ids are allocated by this store and always fit; an out-of-range id simply matches nothing.
    i64::try_from(id.0).unwrap_or(-1)
}

fn corrupt(what: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, what.into(), rusqlite::types::Type::Null)
}

/// Reads a row selected with [`META_COLUMNS`]; tags are filled in separately.
fn meta_from_row(row: &Row<'_>) -> rusqlite::Result<ClipMeta> {
    let id: i64 = row.get(0)?;
    let hash: Vec<u8> = row.get(1)?;
    let mime: String = row.get(2)?;
    let size: i64 = row.get(3)?;
    let hash: [u8; 32] = hash.try_into().map_err(|_| corrupt("hash"))?;
    Ok(ClipMeta {
        id: ClipId(u64::try_from(id).map_err(|_| corrupt("id"))?),
        mime: Mime::from_stored(&mime).map_err(|_| corrupt("mime"))?,
        hash: ContentHash(hash),
        size: u64::try_from(size).map_err(|_| corrupt("size"))?,
        created: Timestamp(row.get(4)?),
        last_used: Timestamp(row.get(5)?),
        preview: row.get(6)?,
        pinned: row.get(7)?,
        tags: TagSet::default(),
    })
}

impl SqliteStore {
    /// Opens (creating and migrating as needed) the session and pinned databases.
    ///
    /// # Errors
    ///
    /// [`StoreError`] on any filesystem or SQLite failure.
    pub fn open(paths: &Paths) -> Result<Self, StoreError> {
        prepare_dir(&paths.session_dir)?;
        prepare_dir(&paths.data_dir)?;
        let session_db = paths.session_db();
        let pinned_db = paths.pinned_db();
        open_and_migrate(&session_db)?;
        open_and_migrate(&pinned_db)?;

        let conn = Connection::open(&session_db)?;
        let pinned_str = pinned_db
            .to_str()
            .ok_or_else(|| StoreError::NonUtf8Path(pinned_db.clone()))?;
        conn.execute("ATTACH DATABASE ?1 AS pinned", [pinned_str])?;
        configure(&conn, SESSION)?;
        configure(&conn, PINNED)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TEMP VIEW all_clips AS
               SELECT id, hash, mime, size, created_ms, last_used_ms, preview, search, data, 0 AS pinned
                 FROM main.clips
               UNION ALL
               SELECT id, hash, mime, size, created_ms, last_used_ms, preview, search, data, 1 AS pinned
                 FROM pinned.clips;
             CREATE TEMP VIEW all_tags AS
               SELECT clip_id, tag FROM main.clip_tags
               UNION ALL
               SELECT clip_id, tag FROM pinned.clip_tags;",
        )?;
        let dropped = conn.execute(
            "DELETE FROM main.clips WHERE hash IN (SELECT hash FROM pinned.clips)",
            [],
        )?;
        if dropped > 0 {
            tracing::warn!(dropped, "removed session clips duplicated in the pinned database");
        }

        let max_id: i64 = conn.query_row(
            "SELECT max(coalesce((SELECT max(id) FROM main.clips), 0),
                        coalesce((SELECT max(id) FROM pinned.clips), 0),
                        coalesce((SELECT value - 1 FROM main.meta WHERE key = 'next_id'), 0))",
            [],
            |r| r.get(0),
        )?;
        let next_id = u64::try_from(max_id).map_err(|_| StoreError::Corrupt("next_id".into()))? + 1;
        tracing::info!(
            session = %session_db.display(),
            pinned = %pinned_db.display(),
            "opened clip store"
        );
        Ok(Self { conn, next_id })
    }

    /// Tier currently holding `id`, if any.
    fn tier_of(&self, id: ClipId) -> Result<Option<&'static str>, StoreError> {
        let pinned: Option<bool> = self
            .conn
            .query_row(
                "SELECT pinned FROM all_clips WHERE id = ?1",
                [id_param(id)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(pinned.map(|p| if p { PINNED } else { SESSION }))
    }

    fn tags_of(&self, id: ClipId) -> Result<TagSet, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT tag FROM all_tags WHERE clip_id = ?1 ORDER BY tag")?;
        let raw: Vec<String> = stmt
            .query_map([id_param(id)], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let tags = raw
            .iter()
            .map(|t| Tag::parse(t).map_err(|e| StoreError::Corrupt(format!("tag: {e}"))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TagSet::from_tags(tags))
    }

    fn insert_tags(tx: &Transaction<'_>, schema: &str, id: i64, tags: &TagSet) -> Result<(), StoreError> {
        let mut stmt = tx.prepare_cached(&format!(
            "INSERT INTO {schema}.clip_tags (clip_id, tag) VALUES (?1, ?2)"
        ))?;
        for tag in tags.iter() {
            stmt.execute(params![id, tag.as_str()])?;
        }
        Ok(())
    }

    /// Builds the `WHERE` clause and parameters for `query` (pagination excluded).
    fn filter_sql(query: &Query) -> (String, Vec<Value>) {
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        if let Some(pinned) = query.scope.pinned_filter() {
            clauses.push("c.pinned = ?".to_owned());
            values.push(Value::Integer(i64::from(pinned)));
        }
        if let Some(needle) = &query.needle {
            clauses.push("c.search IS NOT NULL AND instr(c.search, ?) > 0".to_owned());
            values.push(Value::Text(needle.clone()));
        }
        for tag in query.tags.iter() {
            clauses.push("EXISTS (SELECT 1 FROM all_tags t WHERE t.clip_id = c.id AND t.tag = ?)".to_owned());
            values.push(Value::Text(tag.as_str().to_owned()));
        }
        let sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        (sql, values)
    }
}

impl ClipStore for SqliteStore {
    type Error = StoreError;

    fn find_by_hash(&self, hash: &ContentHash) -> Result<Option<ClipId>, StoreError> {
        let id: Option<i64> = self
            .conn
            .prepare_cached("SELECT id FROM all_clips WHERE hash = ?1 LIMIT 1")?
            .query_row([&hash.0[..]], |r| r.get(0))
            .optional()?;
        id.map(|i| {
            u64::try_from(i)
                .map(ClipId)
                .map_err(|_| StoreError::Corrupt("id".into()))
        })
        .transpose()
    }

    fn insert(&mut self, clip: &Candidate, now: Timestamp) -> Result<ClipId, StoreError> {
        let id = self.next_id;
        let id_sql = to_i64(id)?;
        let next = to_i64(id + 1)?;
        let tx = self.conn.transaction()?;
        tx.prepare_cached(
            "INSERT INTO main.clips (id, hash, mime, size, created_ms, last_used_ms, preview, search, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            id_sql,
            &clip.hash.0[..],
            clip.mime.as_str(),
            to_i64(clip.size())?,
            now.0,
            clip.preview,
            clip.search_text,
            clip.data,
        ])?;
        tx.execute(
            "INSERT INTO main.meta (key, value) VALUES ('next_id', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [next],
        )?;
        tx.commit()?;
        self.next_id = id + 1;
        Ok(ClipId(id))
    }

    fn touch(&mut self, id: ClipId, now: Timestamp) -> Result<bool, StoreError> {
        let Some(schema) = self.tier_of(id)? else {
            return Ok(false);
        };
        let n = self.conn.execute(
            &format!("UPDATE {schema}.clips SET last_used_ms = ?1 WHERE id = ?2"),
            params![now.0, id_param(id)],
        )?;
        Ok(n > 0)
    }

    fn meta(&self, id: ClipId) -> Result<Option<ClipMeta>, StoreError> {
        let meta = self
            .conn
            .prepare_cached(&format!("SELECT {META_COLUMNS} FROM all_clips WHERE id = ?1"))?
            .query_row([id_param(id)], meta_from_row)
            .optional()?;
        match meta {
            Some(mut m) => {
                m.tags = self.tags_of(id)?;
                Ok(Some(m))
            }
            None => Ok(None),
        }
    }

    fn content(&self, id: ClipId) -> Result<Option<(Mime, Vec<u8>)>, StoreError> {
        let row: Option<(String, Vec<u8>)> = self
            .conn
            .prepare_cached("SELECT mime, data FROM all_clips WHERE id = ?1")?
            .query_row([id_param(id)], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        row.map(|(mime, data)| {
            Mime::from_stored(&mime)
                .map(|m| (m, data))
                .map_err(|_| StoreError::Corrupt("mime".into()))
        })
        .transpose()
    }

    fn list(&self, query: &Query) -> Result<Page, StoreError> {
        let (filter, mut values) = Self::filter_sql(query);
        let total: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM all_clips c {filter}"),
            params_from_iter(values.iter()),
            |r| r.get(0),
        )?;
        values.push(Value::Integer(i64::from(query.limit)));
        values.push(Value::Integer(i64::from(query.offset)));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {META_COLUMNS} FROM all_clips c {filter}
             ORDER BY c.last_used_ms DESC, c.id DESC LIMIT ? OFFSET ?"
        ))?;
        let mut clips = stmt
            .query_map(params_from_iter(values.iter()), meta_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        for clip in &mut clips {
            clip.tags = self.tags_of(clip.id)?;
        }
        Ok(Page {
            total: u32::try_from(total).unwrap_or(u32::MAX),
            clips,
        })
    }

    fn set_pinned(&mut self, id: ClipId, pinned: bool) -> Result<bool, StoreError> {
        let Some(current) = self.tier_of(id)? else {
            return Ok(false);
        };
        let (from, to) = if pinned {
            (SESSION, PINNED)
        } else {
            (PINNED, SESSION)
        };
        if current == to {
            return Ok(true);
        }
        let id = id_param(id);
        let tx = self.conn.transaction()?;
        tx.execute(
            &format!(
                "INSERT INTO {to}.clips (id, hash, mime, size, created_ms, last_used_ms, preview, search, data)
                 SELECT id, hash, mime, size, created_ms, last_used_ms, preview, search, data
                   FROM {from}.clips WHERE id = ?1"
            ),
            [id],
        )?;
        tx.execute(
            &format!(
                "INSERT INTO {to}.clip_tags (clip_id, tag) SELECT clip_id, tag FROM {from}.clip_tags WHERE clip_id = ?1"
            ),
            [id],
        )?;
        tx.execute(&format!("DELETE FROM {from}.clips WHERE id = ?1"), [id])?;
        tx.commit()?;
        Ok(true)
    }

    fn set_tags(&mut self, id: ClipId, tags: &TagSet) -> Result<bool, StoreError> {
        let Some(schema) = self.tier_of(id)? else {
            return Ok(false);
        };
        let id = id_param(id);
        let tx = self.conn.transaction()?;
        tx.execute(
            &format!("DELETE FROM {schema}.clip_tags WHERE clip_id = ?1"),
            [id],
        )?;
        Self::insert_tags(&tx, schema, id, tags)?;
        tx.commit()?;
        Ok(true)
    }

    fn delete(&mut self, ids: &[ClipId]) -> Result<u32, StoreError> {
        let tx = self.conn.transaction()?;
        let mut deleted = 0usize;
        {
            let mut session = tx.prepare_cached("DELETE FROM main.clips WHERE id = ?1")?;
            let mut pinned = tx.prepare_cached("DELETE FROM pinned.clips WHERE id = ?1")?;
            for id in ids {
                deleted += session.execute([id_param(*id)])?;
                deleted += pinned.execute([id_param(*id)])?;
            }
        }
        tx.commit()?;
        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }

    fn delete_unpinned(&mut self) -> Result<u32, StoreError> {
        let n = self.conn.execute("DELETE FROM main.clips", [])?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    fn tag_counts(&self) -> Result<Vec<(Tag, u32)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT tag, count(*) FROM all_tags GROUP BY tag ORDER BY tag")?;
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(t, n)| {
                let tag = Tag::parse(&t).map_err(|e| StoreError::Corrupt(format!("tag: {e}")))?;
                Ok((tag, u32::try_from(n).unwrap_or(u32::MAX)))
            })
            .collect()
    }

    fn unpinned_count(&self) -> Result<usize, StoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM main.clips", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    fn oldest_unpinned(&self, n: usize, protect: Option<ClipId>) -> Result<Vec<ClipId>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM main.clips WHERE id != ?2 ORDER BY last_used_ms ASC, id ASC LIMIT ?1",
        )?;
        let limit = i64::try_from(n).unwrap_or(i64::MAX);
        let protect = protect.map_or(-1, id_param);
        let ids: Vec<i64> = stmt
            .query_map(params![limit, protect], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids
            .into_iter()
            .filter_map(|i| u64::try_from(i).ok().map(ClipId))
            .collect())
    }
}
