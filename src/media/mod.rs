//! Media helpers: voice notes (Ogg/Opus), image thumbnails and the
//! byte-budgeted media cache. No UI types here.

pub mod audio;
pub mod avatar;
pub mod image;
pub mod video;
pub mod video_proto;

use std::borrow::Borrow;
use std::hash::Hash;

use crate::error::{Error, Result};

/// Largest media blob Ferrochat downloads (WhatsApp's own document limit is 2 GB;
/// a desktop cache keeps a tighter bound).
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;
/// Largest decoded image accepted when the user opens it (pixels).
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
/// Largest image decoded automatically while scrolling a chat (pixels).
pub const MAX_AUTO_IMAGE_PIXELS: u64 = 16_000_000;
/// Concurrent media downloads.
pub const MAX_CONCURRENT_DOWNLOADS: usize = 4;

/// Rejects downloads above [`MAX_DOWNLOAD_BYTES`].
pub fn check_download_size(len: u64) -> Result<()> {
    if len > MAX_DOWNLOAD_BYTES {
        return Err(Error::Media(format!(
            "media too large ({len} bytes, limit {MAX_DOWNLOAD_BYTES})"
        )));
    }
    Ok(())
}

/// LRU cache bounded by total bytes (architecture §3: decoded images live in
/// an LRU with a byte budget, default 48 MB).
pub struct ByteLru<K: Hash + Eq, V> {
    inner: lru::LruCache<K, (V, usize)>,
    bytes: usize,
    budget: usize,
}

impl<K: Hash + Eq, V> ByteLru<K, V> {
    /// Empty cache with a byte budget.
    pub fn new(budget: usize) -> Self {
        Self {
            inner: lru::LruCache::unbounded(),
            bytes: 0,
            budget,
        }
    }

    /// Inserts `value` of `size` bytes, evicting least-recently-used entries.
    pub fn put(&mut self, key: K, value: V, size: usize) {
        if size > self.budget {
            // Never keep a single entry that alone exceeds the budget.
            if let Some((_, old)) = self.inner.pop(&key) {
                self.bytes -= old;
            }
            return;
        }
        if let Some((_, old)) = self.inner.put(key, (value, size)) {
            self.bytes -= old;
        }
        self.bytes += size;
        while self.bytes > self.budget {
            if let Some((_, (_, s))) = self.inner.pop_lru() {
                self.bytes -= s;
            }
        }
    }

    /// Looks up and marks as recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.inner.get(key).map(|(v, _)| v)
    }

    /// Removes an entry.
    pub fn pop<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let (v, size) = self.inner.pop(key)?;
        self.bytes -= size;
        Some(v)
    }

    /// Looks up without touching recency (usable from `&self` views).
    pub fn peek<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.inner.peek(key).map(|(v, _)| v)
    }

    /// True if present.
    pub fn contains<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.inner.contains(key)
    }

    /// Bytes currently held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Drops everything.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_cap() {
        assert!(check_download_size(10).is_ok());
        assert!(check_download_size(MAX_DOWNLOAD_BYTES + 1).is_err());
    }

    #[test]
    fn evicts_by_bytes() {
        let mut c = ByteLru::new(100);
        c.put("a", 1, 40);
        c.put("b", 2, 40);
        assert_eq!(c.get(&"a"), Some(&1)); // a is now most recent
        c.put("c", 3, 40);
        assert!(c.contains(&"a"));
        assert!(!c.contains(&"b"));
        assert_eq!(c.bytes(), 80);
        c.put("a", 9, 10);
        assert_eq!(c.bytes(), 50);
        assert_eq!(c.peek(&"a"), Some(&9));
        // An entry bigger than the whole budget is not kept (and replaces nothing).
        c.put("huge", 1, 101);
        assert!(!c.contains(&"huge"));
        c.put("a", 2, 500);
        assert!(!c.contains(&"a"));
        assert_eq!(c.bytes(), 40);
    }
}
