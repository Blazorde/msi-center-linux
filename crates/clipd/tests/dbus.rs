//! D-Bus round trip over a private peer-to-peer connection (a socket pair): no session bus needed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::net::UnixStream;
use std::time::Duration;

use clip_core::content::DEFAULT_MAX_CLIP_BYTES;
use clip_core::{Library, Limits};
use clipd::{Clipd, OBJECT_PATH, Paths, SqliteStore};
use futures_util::StreamExt as _;
use tempfile::TempDir;
use zbus::connection::Builder;

type Entry = (u64, String, String, u64, i64, i64, bool, Vec<String>);

#[zbus::proxy(
    interface = "org.blazorde.Clipd1",
    default_service = "org.blazorde.Clipd1",
    default_path = "/org/blazorde/Clipd"
)]
trait Clipd1 {
    fn add(&self, mime: &str, data: &[u8]) -> zbus::Result<u64>;
    fn list(
        &self,
        query: &str,
        tags: &[&str],
        scope: &str,
        offset: u32,
        limit: u32,
    ) -> zbus::Result<(u32, Vec<Entry>)>;
    fn get(&self, id: u64) -> zbus::Result<(String, Vec<u8>)>;
    fn pin(&self, id: u64) -> zbus::Result<()>;
    fn unpin(&self, id: u64) -> zbus::Result<()>;
    fn delete(&self, ids: &[u64]) -> zbus::Result<u32>;
    fn clear_unpinned(&self) -> zbus::Result<u32>;
    fn set_tags(&self, id: u64, tags: &[&str]) -> zbus::Result<Vec<String>>;
    fn tags(&self) -> zbus::Result<Vec<(String, u32)>>;
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn max_unpinned(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn max_clip_bytes(&self) -> zbus::Result<u64>;
}

struct Harness {
    _dir: TempDir,
    _server: zbus::Connection,
    _client: zbus::Connection,
    proxy: Clipd1Proxy<'static>,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path().join("run/clipd"), dir.path().join("data/clipd"));
    let lib = Library::new(SqliteStore::open(&paths).unwrap(), Limits::default());
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(OBJECT_PATH, Clipd::new(lib))
        .build();
    let client = Builder::unix_stream(b).p2p().build();
    let (server, client) = tokio::try_join!(server, client).unwrap();
    let proxy = Clipd1Proxy::new(&client).await.unwrap();
    Harness {
        _dir: dir,
        _server: server,
        _client: client,
        proxy,
    }
}

fn error_name(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => panic!("expected a method error, got {other:?}"),
    }
}

#[tokio::test]
async fn round_trip() {
    let h = harness().await;
    let p = &h.proxy;

    assert_eq!(p.max_unpinned().await.unwrap(), 500);
    assert_eq!(p.max_clip_bytes().await.unwrap(), DEFAULT_MAX_CLIP_BYTES as u64);

    let a = p
        .add("text/plain;charset=utf-8", "Hello\nWorld".as_bytes())
        .await
        .unwrap();
    let img = p.add("image/png", &[0x89, b'P', b'N', b'G']).await.unwrap();
    assert_eq!(
        p.add("text/plain", b"Hello\nWorld").await.unwrap(),
        a,
        "dedup returns the same id"
    );

    let (total, clips) = p.list("", &[], "", 0, 0).await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(clips[0].0, a, "bumped clip comes first");
    assert_eq!(clips[0].1, "text/plain");
    assert_eq!(clips[0].2, "Hello World");
    assert_eq!(clips[0].3, 11);
    assert!(!clips[0].6);
    assert_eq!(clips[1].0, img);

    assert_eq!(
        p.get(img).await.unwrap(),
        ("image/png".to_owned(), vec![0x89, b'P', b'N', b'G'])
    );

    assert_eq!(
        p.set_tags(a, &[" Work ", "work", "Code"]).await.unwrap(),
        vec!["code", "work"]
    );
    assert_eq!(
        p.tags().await.unwrap(),
        vec![("code".to_owned(), 1), ("work".to_owned(), 1)]
    );
    let (total, clips) = p.list("world", &["WORK"], "all", 0, 10).await.unwrap();
    assert_eq!((total, clips[0].0), (1, a));
    assert_eq!(clips[0].7, vec!["code", "work"]);

    p.pin(a).await.unwrap();
    let (_, pinned) = p.list("", &[], "pinned", 0, 0).await.unwrap();
    assert_eq!(
        pinned.iter().map(|c| (c.0, c.6)).collect::<Vec<_>>(),
        vec![(a, true)]
    );
    p.unpin(a).await.unwrap();
    let (n, _) = p.list("", &[], "pinned", 0, 0).await.unwrap();
    assert_eq!(n, 0);

    assert_eq!(p.delete(&[a, img, 999]).await.unwrap(), 2);
    assert_eq!(p.list("", &[], "", 0, 0).await.unwrap().0, 0);

    p.add("text/plain", b"x").await.unwrap();
    assert_eq!(p.clear_unpinned().await.unwrap(), 1);
}

#[tokio::test]
async fn errors_have_stable_names() {
    let h = harness().await;
    let p = &h.proxy;
    let e = p.get(42).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.NotFound");
    let e = p.pin(42).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.NotFound");
    let e = p.add("application/pdf", b"x").await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.Rejected");
    let e = p.add("text/plain", b"").await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.Rejected");
    let e = p.list("", &[], "everything", 0, 0).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.InvalidArgument");
    let id = p.add("text/plain", b"x").await.unwrap();
    let e = p.set_tags(id, &["bad\u{1}tag"]).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.InvalidArgument");
}

#[tokio::test]
async fn oversized_image_is_rejected_over_dbus() {
    let h = harness().await;
    let p = &h.proxy;
    let e = p
        .add("image/png", &vec![0u8; DEFAULT_MAX_CLIP_BYTES + 1])
        .await
        .unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.Clipd1.Error.Rejected");
    assert_eq!(p.list("", &[], "", 0, 0).await.unwrap().0, 0);
}

#[tokio::test]
async fn changed_signal_is_emitted() {
    let h = harness().await;
    let p = &h.proxy;
    let mut changes = p.receive_changed().await.unwrap();
    let id = p.add("text/plain", b"hello").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), changes.next())
        .await
        .unwrap()
        .unwrap();
    p.pin(id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), changes.next())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn set_tags_pins_and_signals_changed() {
    let h = harness().await;
    let p = &h.proxy;
    let id = p.add("text/plain", b"keep me").await.unwrap();
    let mut changes = p.receive_changed().await.unwrap();
    assert_eq!(p.set_tags(id, &["Work"]).await.unwrap(), vec!["work"]);
    tokio::time::timeout(Duration::from_secs(5), changes.next())
        .await
        .unwrap()
        .unwrap();
    let (_, pinned) = p.list("", &[], "pinned", 0, 0).await.unwrap();
    assert_eq!(
        pinned.iter().map(|c| (c.0, c.6, c.7.clone())).collect::<Vec<_>>(),
        vec![(id, true, vec!["work".to_owned()])]
    );
    // Clearing the tags does not unpin; an explicit Unpin does and keeps the tags.
    assert_eq!(p.set_tags(id, &[]).await.unwrap(), Vec::<String>::new());
    assert_eq!(p.list("", &[], "pinned", 0, 0).await.unwrap().0, 1);
    p.set_tags(id, &["work"]).await.unwrap();
    p.unpin(id).await.unwrap();
    let (_, clips) = p.list("", &["work"], "unpinned", 0, 0).await.unwrap();
    assert_eq!(clips.iter().map(|c| c.0).collect::<Vec<_>>(), vec![id]);
}

#[tokio::test]
async fn introspection_matches_documented_signatures() {
    let h = harness().await;
    let intro = zbus::fdo::IntrospectableProxy::builder(h.proxy.inner().connection())
        .destination("org.blazorde.Clipd1")
        .path(OBJECT_PATH)
        .build()
        .await
        .unwrap();
    let xml = intro.introspect().await.unwrap();
    for fragment in [
        r#"<interface name="org.blazorde.Clipd1">"#,
        r#"<arg name="clips" type="a(tsstxxbas)" direction="out"/>"#,
        r#"<arg name="total" type="u" direction="out"/>"#,
        r#"<arg name="data" type="ay" direction="out"/>"#,
        r#"<arg name="ids" type="at" direction="in"/>"#,
        r#"<arg name="id" type="t" direction="out"/>"#,
        r#"<arg name="deleted" type="u" direction="out"/>"#,
        r#"<arg name="tags" type="a(su)" direction="out"/>"#,
        r#"<signal name="Changed">"#,
        r#"<property name="MaxUnpinned" type="u" access="read"/>"#,
        r#"<property name="MaxClipBytes" type="t" access="read"/>"#,
    ] {
        assert!(xml.contains(fragment), "missing {fragment}");
    }
    // SetTags keeps `(t id, as tags) -> as tags` although it now pins tagged clips.
    let start = xml.find(r#"<method name="SetTags">"#).expect("SetTags");
    let block = &xml[start..start + xml[start..].find("</method>").unwrap()];
    for fragment in [
        r#"<arg name="id" type="t" direction="in"/>"#,
        r#"<arg name="tags" type="as" direction="in"/>"#,
        r#"<arg name="tags" type="as" direction="out"/>"#,
    ] {
        assert!(block.contains(fragment), "SetTags: missing {fragment}");
    }
    assert_eq!(block.matches("<arg ").count(), 3, "SetTags: {block}");
}
