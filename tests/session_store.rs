//! The protocol library's Signal/session store is encrypted at rest with
//! SQLCipher, keyed from the (in-memory, in tests) secret store.

use ferrochat::auth::secrets::{MemoryStore, signal_store_key};
use ferrochat::net::whatsapp::{open_store, remove_db};

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ferrochat-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("session.sqlcipher")
}

#[tokio::test]
async fn session_store_is_encrypted_on_disk() {
    let path = tmp("enc");
    let secrets = MemoryStore::new();
    let (key, created) = signal_store_key(&secrets).unwrap();
    assert!(created);
    {
        let store = open_store(&path, key.expose().to_string()).await.unwrap();
        // Force the library to write its device row (identity keys).
        let dev = store.create_new_device().await.unwrap();
        assert!(dev >= 1);
    }
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
    assert!(open_store(&path, key.expose().to_string()).await.is_ok());
    // Refuses a wrong key, quickly.
    let wrong = "00".repeat(32);
    let t = std::time::Instant::now();
    assert!(open_store(&path, wrong).await.is_err());
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    remove_db(&path);
    assert!(!path.exists());
}
