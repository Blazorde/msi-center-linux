//! Integration tests of the SQLite store through `clip_core::Library`, against temporary directories
//! (never the real XDG directories). "Logout" is simulated by deleting the session directory, which is
//! what logind does to `$XDG_RUNTIME_DIR`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use clip_core::content::DEFAULT_MAX_CLIP_BYTES;
use clip_core::{
    AddKind, ClipId, ClipStore, Library, LibraryError, Limits, MemoryStore, Query, RejectReason, Scope,
    Timestamp,
};
use clipd::{Paths, SqliteStore};
use proptest::prelude::*;
use tempfile::TempDir;

struct Env {
    _root: TempDir,
    paths: Paths,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(root.path().join("run/clipd"), root.path().join("data/clipd"));
        Self { _root: root, paths }
    }

    fn open(&self) -> Library<SqliteStore> {
        self.open_with(Limits::default())
    }

    fn open_with(&self, limits: Limits) -> Library<SqliteStore> {
        Library::new(SqliteStore::open(&self.paths).unwrap(), limits)
    }

    fn logout(&self) {
        std::fs::remove_dir_all(&self.paths.session_dir).unwrap();
    }
}

fn all() -> Query {
    let none: [&str; 0] = [];
    Query::new("", &none, Scope::All, 0, 1000).unwrap()
}

fn query(text: &str, tags: &[&str], scope: Scope) -> Query {
    Query::new(text, tags, scope, 0, 1000).unwrap()
}

fn ids<S: ClipStore>(lib: &Library<S>, q: &Query) -> Vec<u64> {
    lib.list(q).unwrap().clips.iter().map(|c| c.id.0).collect()
}

fn text(lib: &mut Library<SqliteStore>, s: &str, t: i64) -> ClipId {
    lib.add("text/plain", s.as_bytes().to_vec(), Timestamp(t))
        .unwrap()
        .id
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn files_and_dirs_are_private() {
    let env = Env::new();
    let _lib = env.open();
    assert_eq!(mode(&env.paths.session_dir), 0o700);
    assert_eq!(mode(&env.paths.data_dir), 0o700);
    assert_eq!(mode(&env.paths.session_db()), 0o600);
    assert_eq!(mode(&env.paths.pinned_db()), 0o600);
}

#[test]
fn existing_files_are_tightened() {
    let env = Env::new();
    drop(env.open());
    std::fs::set_permissions(env.paths.pinned_db(), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&env.paths.data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    drop(env.open());
    assert_eq!(mode(&env.paths.pinned_db()), 0o600);
    assert_eq!(mode(&env.paths.data_dir), 0o700);
}

#[test]
fn pinned_survives_logout_unpinned_does_not() {
    let env = Env::new();
    let mut lib = env.open();
    let keep = text(&mut lib, "keep me", 1);
    let lose = text(&mut lib, "forget me", 2);
    lib.pin(keep).unwrap();
    lib.set_tags(keep, &["Work"]).unwrap();
    drop(lib);

    env.logout();
    let lib = env.open();
    assert_eq!(ids(&lib, &all()), vec![keep.0]);
    let meta = lib.store().meta(keep).unwrap().unwrap();
    assert!(meta.pinned);
    assert_eq!(meta.tags.to_strings(), vec!["work"]);
    assert_eq!(lib.get(keep).unwrap().1, b"keep me");
    assert!(matches!(lib.get(lose), Err(LibraryError::NotFound(_))));
}

#[test]
fn tagged_clip_is_pinned_and_survives_logout() {
    let env = Env::new();
    let mut lib = env.open();
    let keep = text(&mut lib, "tagged, never pinned by hand", 1);
    let lose = text(&mut lib, "untagged", 2);
    let out = lib.set_tags(keep, &[" Work "]).unwrap();
    assert!(out.pinned);
    assert_eq!(ids(&lib, &query("", &[], Scope::Pinned)), vec![keep.0]);
    drop(lib);

    let disk = std::fs::read(env.paths.pinned_db()).unwrap();
    let needle = b"tagged, never pinned by hand";
    assert!(
        disk.windows(needle.len()).any(|w| w == needle),
        "moved to the persistent DB"
    );

    env.logout();
    let lib = env.open();
    assert_eq!(ids(&lib, &all()), vec![keep.0]);
    let meta = lib.store().meta(keep).unwrap().unwrap();
    assert!(meta.pinned);
    assert_eq!(meta.tags.to_strings(), vec!["work"]);
    assert_eq!(lib.get(keep).unwrap().1, needle);
    assert!(matches!(lib.get(lose), Err(LibraryError::NotFound(_))));
}

#[test]
fn unpinned_tagged_clip_keeps_tags_and_is_lost_at_logout() {
    let env = Env::new();
    let mut lib = env.open();
    let id = text(&mut lib, "a", 1);
    lib.set_tags(id, &["work"]).unwrap();
    lib.unpin(id).unwrap();
    let meta = lib.store().meta(id).unwrap().unwrap();
    assert!(!meta.pinned);
    assert_eq!(meta.tags.to_strings(), vec!["work"]);
    assert_eq!(ids(&lib, &query("", &["work"], Scope::Unpinned)), vec![id.0]);
    drop(lib);
    env.logout();
    let lib = env.open();
    assert_eq!(ids(&lib, &all()), Vec::<u64>::new());
    assert_eq!(lib.tags().unwrap(), Vec::new());
}

#[test]
fn clearing_tags_keeps_the_clip_pinned() {
    let env = Env::new();
    let mut lib = env.open();
    let id = text(&mut lib, "a", 1);
    lib.set_tags(id, &["work"]).unwrap();
    let none: [&str; 0] = [];
    assert!(!lib.set_tags(id, &none).unwrap().pinned);
    drop(lib);
    env.logout();
    let lib = env.open();
    let meta = lib.store().meta(id).unwrap().unwrap();
    assert!(meta.pinned);
    assert!(meta.tags.is_empty());
}

#[test]
fn unpinned_survives_a_daemon_restart_within_the_session() {
    let env = Env::new();
    let mut lib = env.open();
    let id = text(&mut lib, "still here", 1);
    drop(lib);
    let lib = env.open();
    assert_eq!(ids(&lib, &all()), vec![id.0]);
}

#[test]
fn unpinned_content_never_written_to_the_data_dir() {
    let env = Env::new();
    let mut lib = env.open();
    text(&mut lib, "secret-in-session-only", 1);
    drop(lib);
    let disk = std::fs::read(env.paths.pinned_db()).unwrap();
    let needle = b"secret-in-session-only";
    assert!(!disk.windows(needle.len()).any(|w| w == needle));
}

#[test]
fn unpinning_moves_back_to_session_and_is_lost_at_logout() {
    let env = Env::new();
    let mut lib = env.open();
    let id = text(&mut lib, "a", 1);
    lib.pin(id).unwrap();
    lib.pin(id).unwrap(); // idempotent
    lib.unpin(id).unwrap();
    lib.unpin(id).unwrap(); // idempotent
    drop(lib);
    env.logout();
    assert_eq!(ids(&env.open(), &all()), Vec::<u64>::new());
}

#[test]
fn deleted_pinned_content_is_wiped_from_disk() {
    let env = Env::new();
    let mut lib = env.open();
    let id = text(&mut lib, "pinned-then-deleted-secret", 1);
    lib.pin(id).unwrap();
    lib.delete(&[id]).unwrap();
    drop(lib);
    let disk = std::fs::read(env.paths.pinned_db()).unwrap();
    let needle = b"pinned-then-deleted-secret";
    assert!(
        !disk.windows(needle.len()).any(|w| w == needle),
        "secure_delete must zero freed pages"
    );
}

#[test]
fn eviction_at_501_keeps_500_and_spares_pinned() {
    let env = Env::new();
    let mut lib = env.open();
    let pinned = text(&mut lib, "pinned", 0);
    lib.pin(pinned).unwrap();
    let mut added = Vec::new();
    for i in 1..=500 {
        let out = lib
            .add("text/plain", format!("clip {i}").into_bytes(), Timestamp(i))
            .unwrap();
        assert!(out.evicted.is_empty(), "no eviction up to 500");
        added.push(out.id);
    }
    let out = lib
        .add("text/plain", b"clip 501".to_vec(), Timestamp(501))
        .unwrap();
    assert_eq!(out.evicted, vec![added[0]]);
    assert_eq!(lib.store().unpinned_count().unwrap(), 500);
    assert!(lib.get(added[0]).is_err());
    assert!(lib.get(pinned).is_ok());
    assert_eq!(lib.list(&all()).unwrap().total, 501);
}

#[test]
fn startup_retention_applies_a_lowered_limit() {
    let env = Env::new();
    let mut lib = env.open();
    for i in 0..5 {
        text(&mut lib, &format!("{i}"), i);
    }
    drop(lib);
    let mut lib = env.open_with(Limits {
        max_unpinned: 2,
        ..Limits::default()
    });
    assert_eq!(lib.enforce_retention().unwrap().len(), 3);
    assert_eq!(lib.store().unpinned_count().unwrap(), 2);
}

#[test]
fn image_size_limit() {
    let env = Env::new();
    let mut lib = env.open();
    let ok = lib
        .add("image/png", vec![7; DEFAULT_MAX_CLIP_BYTES], Timestamp(1))
        .unwrap();
    assert_eq!(lib.get(ok.id).unwrap().1.len(), DEFAULT_MAX_CLIP_BYTES);
    let err = lib
        .add("image/png", vec![8; DEFAULT_MAX_CLIP_BYTES + 1], Timestamp(2))
        .unwrap_err();
    assert!(matches!(
        err,
        LibraryError::Rejected(RejectReason::TooLarge { .. })
    ));
    assert_eq!(lib.list(&all()).unwrap().total, 1);
}

#[test]
fn dedup_bumps_across_tiers() {
    let env = Env::new();
    let mut lib = env.open();
    let a = text(&mut lib, "same", 1);
    let b = text(&mut lib, "other", 2);
    let again = lib
        .add("text/plain;charset=utf-8", b"same".to_vec(), Timestamp(3))
        .unwrap();
    assert_eq!((again.id, again.kind), (a, AddKind::Bumped));
    assert_eq!(ids(&lib, &all()), vec![a.0, b.0]);

    lib.pin(a).unwrap();
    let again = lib.add("text/plain", b"same".to_vec(), Timestamp(4)).unwrap();
    assert_eq!((again.id, again.kind), (a, AddKind::Bumped));
    assert_eq!(lib.list(&all()).unwrap().total, 2);
    assert!(lib.store().meta(a).unwrap().unwrap().pinned);
}

#[test]
fn search_and_tag_filters() {
    let env = Env::new();
    let mut lib = env.open();
    let a = text(&mut lib, "Bonjour le Monde", 1);
    let b = text(&mut lib, "hello world", 2);
    let c = text(&mut lib, "ÉTÉ indien", 3);
    let img = lib.add("image/png", vec![1, 2, 3], Timestamp(4)).unwrap().id;
    lib.set_tags(a, &["fr", "greeting"]).unwrap();
    lib.set_tags(b, &["EN", "Greeting "]).unwrap();
    lib.set_tags(img, &["greeting"]).unwrap();
    // Tagging pinned all three; unpin two so the filters run across both tiers (tags are kept).
    lib.unpin(a).unwrap();
    lib.unpin(img).unwrap();

    assert_eq!(ids(&lib, &query("MONDE", &[], Scope::All)), vec![a.0]);
    assert_eq!(ids(&lib, &query("o", &[], Scope::All)), vec![b.0, a.0]);
    assert_eq!(ids(&lib, &query("été", &[], Scope::All)), vec![c.0]);
    assert_eq!(
        ids(&lib, &query("", &["greeting"], Scope::All)),
        vec![img.0, b.0, a.0]
    );
    assert_eq!(ids(&lib, &query("", &["greeting", "fr"], Scope::All)), vec![a.0]);
    assert_eq!(ids(&lib, &query("", &["greeting"], Scope::Pinned)), vec![b.0]);
    assert_eq!(
        ids(&lib, &query("", &["greeting"], Scope::Unpinned)),
        vec![img.0, a.0]
    );
    assert_eq!(ids(&lib, &query("hello", &["fr"], Scope::All)), Vec::<u64>::new());

    let tags: Vec<(String, u32)> = lib
        .tags()
        .unwrap()
        .into_iter()
        .map(|(t, n)| (t.as_str().to_owned(), n))
        .collect();
    assert_eq!(
        tags,
        vec![("en".into(), 1), ("fr".into(), 1), ("greeting".into(), 3)]
    );
}

#[test]
fn pagination_and_total() {
    let env = Env::new();
    let mut lib = env.open();
    for i in 0..10 {
        text(&mut lib, &format!("item {i}"), i);
    }
    let none: [&str; 0] = [];
    let page = lib
        .list(&Query::new("item", &none, Scope::All, 2, 3).unwrap())
        .unwrap();
    assert_eq!(page.total, 10);
    assert_eq!(
        page.clips.iter().map(|c| c.preview.as_str()).collect::<Vec<_>>(),
        ["item 7", "item 6", "item 5"]
    );
}

#[test]
fn bulk_delete_and_clear() {
    let env = Env::new();
    let mut lib = env.open();
    let a = text(&mut lib, "a", 1);
    let b = text(&mut lib, "b", 2);
    let c = text(&mut lib, "c", 3);
    let d = text(&mut lib, "d", 4);
    lib.pin(a).unwrap();
    lib.set_tags(b, &["x"]).unwrap();
    assert_eq!(lib.delete(&[a, b, ClipId(12345)]).unwrap(), 2);
    assert_eq!(ids(&lib, &all()), vec![d.0, c.0]);
    assert!(lib.tags().unwrap().is_empty(), "tags cascade with their clip");
    assert_eq!(lib.clear_unpinned().unwrap(), 2);
    assert_eq!(ids(&lib, &all()), Vec::<u64>::new());
}

#[test]
fn ids_are_stable_across_pin_and_not_reused() {
    let env = Env::new();
    let mut lib = env.open();
    let a = text(&mut lib, "a", 1);
    let b = text(&mut lib, "b", 2);
    lib.pin(a).unwrap();
    lib.delete(&[b]).unwrap();
    drop(lib);
    // Restart: the deleted id is not handed out again.
    let mut lib = env.open();
    let c = text(&mut lib, "c", 3);
    assert!(c > b);
    drop(lib);
    // Logout: new ids start above every pinned id.
    env.logout();
    let mut lib = env.open();
    let d = text(&mut lib, "d", 4);
    assert!(d > a);
    assert_eq!(lib.get(a).unwrap().1, b"a");
}

#[test]
fn not_found_errors() {
    let env = Env::new();
    let mut lib = env.open();
    assert!(matches!(lib.pin(ClipId(9)), Err(LibraryError::NotFound(_))));
    assert!(matches!(lib.unpin(ClipId(9)), Err(LibraryError::NotFound(_))));
    assert!(matches!(lib.get(ClipId(9)), Err(LibraryError::NotFound(_))));
    assert!(matches!(
        lib.set_tags(ClipId(9), &["a"]),
        Err(LibraryError::NotFound(_))
    ));
    assert!(matches!(
        lib.get(ClipId(u64::MAX)),
        Err(LibraryError::NotFound(_))
    ));
}

#[derive(Debug, Clone)]
enum Op {
    Add(u8),
    Pin(u8),
    Unpin(u8),
    Delete(u8),
    Tag(u8, u8),
    Clear,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (0u8..12).prop_map(Op::Add),
        2 => (1u8..16).prop_map(Op::Pin),
        1 => (1u8..16).prop_map(Op::Unpin),
        1 => (1u8..16).prop_map(Op::Delete),
        2 => ((1u8..16), (0u8..3)).prop_map(|(i, t)| Op::Tag(i, t)),
        1 => Just(Op::Clear),
    ]
}

fn apply<S: ClipStore>(lib: &mut Library<S>, op: &Op, now: Timestamp) -> String {
    let r = match op {
        Op::Add(v) => lib
            .add("text/plain", vec![b'a' + v], now)
            .map(|o| o.id.0.to_string()),
        Op::Pin(i) => lib.pin(ClipId((*i).into())).map(|()| String::new()),
        Op::Unpin(i) => lib.unpin(ClipId((*i).into())).map(|e| format!("{e:?}")),
        Op::Delete(i) => lib.delete(&[ClipId((*i).into())]).map(|n| n.to_string()),
        Op::Tag(i, t) => lib
            .set_tags(ClipId((*i).into()), &[format!("t{t}")])
            .map(|s| format!("{s:?}")),
        Op::Clear => lib.clear_unpinned().map(|n| n.to_string()),
    };
    r.unwrap_or_else(|e| format!("err: {e}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The SQLite store behaves exactly like the in-memory reference model.
    #[test]
    fn sqlite_matches_reference_model(ops in prop::collection::vec(op(), 1..60)) {
        let env = Env::new();
        let limits = Limits { max_unpinned: 4, ..Limits::default() };
        let mut sql = env.open_with(limits);
        let mut mem = Library::new(MemoryStore::new(), limits);
        for (t, op) in ops.iter().enumerate() {
            let now = Timestamp(i64::try_from(t).unwrap());
            prop_assert_eq!(apply(&mut sql, op, now), apply(&mut mem, op, now), "op {:?}", op);
            prop_assert_eq!(sql.list(&all()).unwrap(), mem.list(&all()).unwrap());
            prop_assert_eq!(sql.tags().unwrap(), mem.tags().unwrap());
        }
    }
}
