//! The protocol library's Signal/session store is encrypted at rest with
//! SQLCipher, keyed from the (in-memory, in tests) secret store, and is never
//! deleted automatically.

use std::sync::Arc;

use ferrochat::auth::secrets::{
    MemoryStore, SIGNAL_STORE_KEY, Secret, SecretStore, signal_store_key,
};
use ferrochat::net::Event;
use ferrochat::net::whatsapp::{SessionError, open_store, prepare_session, remove_db};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ferrochat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("session.sqlcipher")
}

fn key(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_string())
}

async fn create_session(path: &std::path::Path, secrets: &MemoryStore) {
    let (k, _) = signal_store_key(secrets).unwrap();
    let store = open_store(path, key(k.expose())).await.unwrap();
    store.create_new_device().await.unwrap();
}

fn events(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<String> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(format!("{e:?}"));
    }
    v
}

#[tokio::test]
async fn session_store_is_encrypted_on_disk() {
    let path = tmp("enc");
    let secrets = MemoryStore::new();
    let (k, created) = signal_store_key(&secrets).unwrap();
    assert!(created);
    create_session(&path, &secrets).await;
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 1024);
    assert!(
        !bytes.starts_with(b"SQLite format 3"),
        "plain SQLite header found"
    );
    // No schema or table names leak in clear text.
    let hay = String::from_utf8_lossy(&bytes);
    assert!(!hay.contains("CREATE TABLE"));

    // Reopens with the right key.
    assert!(open_store(&path, key(k.expose())).await.is_ok());
    // A wrong key is classified as a mismatch, quickly, and nothing is deleted.
    let t = std::time::Instant::now();
    let r = open_store(&path, key(&"00".repeat(32))).await;
    assert!(matches!(r, Err(SessionError::KeyMismatch)));
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    assert!(path.exists());
    remove_db(&path).unwrap();
    assert!(!path.exists());
    // Removing again is fine (missing files are not errors).
    remove_db(&path).unwrap();
}

#[tokio::test]
async fn wrong_key_asks_the_user_and_keeps_the_file() {
    let path = tmp("mismatch");
    let secrets = MemoryStore::new();
    create_session(&path, &secrets).await;
    // Simulate a replaced keyring entry.
    secrets
        .set(SIGNAL_STORE_KEY, &Secret::new("ab".repeat(32)))
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = path.parent().unwrap().to_path_buf();
    let r = prepare_session(&dir, &path, Arc::new(secrets.clone()), &tx)
        .await
        .unwrap();
    assert!(r.is_none());
    assert!(events(&mut rx).iter().any(|e| e == "SessionUnreadable"));
    assert!(path.exists(), "session must not be wiped automatically");
}

#[tokio::test]
async fn missing_key_does_not_mint_a_new_one_over_a_session() {
    let path = tmp("nokey");
    let secrets = MemoryStore::new();
    create_session(&path, &secrets).await;
    secrets.delete(SIGNAL_STORE_KEY).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = path.parent().unwrap().to_path_buf();
    let r = prepare_session(&dir, &path, Arc::new(secrets.clone()), &tx)
        .await
        .unwrap();
    assert!(r.is_none());
    assert!(events(&mut rx).iter().any(|e| e == "SessionUnreadable"));
    assert!(
        secrets.get(SIGNAL_STORE_KEY).unwrap().is_none(),
        "no new key minted"
    );
    assert!(path.exists());
}

#[tokio::test]
async fn io_errors_never_wipe_the_session() {
    // The "database file" is a directory: opening fails with an I/O-class
    // error (SQLITE_CANTOPEN), which must surface as an error, not a wipe.
    let path = tmp("io");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("marker"), b"keep me").unwrap();
    let secrets = MemoryStore::new();
    signal_store_key(&secrets).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = path.parent().unwrap().to_path_buf();
    let r = prepare_session(&dir, &path, Arc::new(secrets), &tx).await;
    assert!(
        r.is_err(),
        "I/O problems are errors (Fatal + retry in the UI)"
    );
    assert!(!events(&mut rx).iter().any(|e| e == "SessionUnreadable"));
    assert!(path.join("marker").exists(), "nothing was deleted");
}

#[tokio::test]
async fn fresh_install_creates_key_and_store() {
    let path = tmp("fresh");
    let secrets = MemoryStore::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = path.parent().unwrap().to_path_buf();
    let r = prepare_session(&dir, &path, Arc::new(secrets.clone()), &tx)
        .await
        .unwrap();
    assert!(r.is_some());
    assert!(events(&mut rx).iter().any(|e| e == "NoSession"));
    assert!(secrets.get(SIGNAL_STORE_KEY).unwrap().is_some());
}
