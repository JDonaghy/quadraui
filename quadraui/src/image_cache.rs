//! Small fixed-capacity LRU cache for decoded/rasterised images, shared by
//! every pixel backend's `Backend::draw_image` (issue #1014).
//!
//! # Why
//!
//! `Image` deliberately carries no cache itself (see
//! [`crate::primitives::image`]'s module docs) — decoding is a backend
//! concern, and each backend's `draw_image` used to decode `image.source`
//! from scratch on *every* paint call. For a large vector source (a 1024²
//! app-icon SVG, vimcode's motivating case) that measured at
//! **+16.5ms/frame** on GTK's `gdk_pixbuf` loader — expensive enough that
//! vimcode's own host code pre-rasterised the icon itself with toolkit
//! types (`gdk_pixbuf`, directly in `src/gtk/util.rs`) just to work
//! around it, behind a `#[cfg(feature = "gui")]` fork in otherwise
//! backend-neutral code. This cache closes that gap generically, inside
//! `draw_image` itself, so every backend gets it for free instead of
//! every consumer reinventing it in host code.
//!
//! # Shape
//!
//! Keyed on a cheap hash of the [`ImageSource`]'s encoded bytes/path, plus
//! the resolved paint size and DPI scale the decode was rasterised at
//! (the issue's own proposed key shape) — two `draw_image` calls for the
//! same source at the same size and scale reuse the same decoded value
//! rather than re-decoding. Capacity is small and fixed: the issue's own
//! scope note is "the sizes in play are UI chrome, not media" — a handful
//! of app icons/toolbar glyphs, not an image gallery — so a small linear
//! LRU is plenty; nothing here is tuned for a large working set.
//!
//! Generic over the decoded value type `T` so each backend can plug in
//! its own native type — `gdk_pixbuf::Pixbuf` on GTK, `CGImage` on macOS,
//! `ID2D1Bitmap` on Win — without this module depending on any of them.
//! A `draw_image` that misses the cache still owns deciding *what* to
//! decode and cache (e.g. GTK caches the already-scaled `Pixbuf`, since
//! `scale_simple` is itself non-trivial cost on top of the decode); this
//! module only owns the lookup/eviction bookkeeping.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::primitives::image::ImageSource;

/// Default capacity for a fresh [`ImageCache`] — see the module docs'
/// "the sizes in play are UI chrome, not media" note.
pub const DEFAULT_CAPACITY: usize = 16;

/// Cache key: [`ImageSource`]'s content hash plus the target size (in
/// device pixels, already rounded by the caller) and DPI scale a decode
/// was rasterised at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    source_hash: u64,
    width: u32,
    height: u32,
    /// `scale * 1000`, rounded — `f32` isn't `Hash`/`Eq`, and DPI scale
    /// only ever needs a few decimal digits of precision (real scale
    /// factors are values like `1.0`, `1.25`, `1.5`, `2.0`).
    scale_millis: u32,
}

impl ImageCacheKey {
    fn new(source: &ImageSource, width: u32, height: u32, scale: f32) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        Self {
            source_hash: hasher.finish(),
            width,
            height,
            scale_millis: (scale.max(0.0) * 1000.0).round() as u32,
        }
    }
}

/// Small fixed-capacity LRU cache mapping an `(ImageSource, size, scale)`
/// key to a backend's decoded/rasterised image type `T`. See the module
/// docs.
pub struct ImageCache<T> {
    capacity: usize,
    // Recency order: front = least recently used, back = most recently
    // used. `capacity` is small (see `DEFAULT_CAPACITY`), so a linear
    // `Vec` scan/rewrite per lookup is cheaper in practice — and much
    // simpler — than an intrusive LRU list at this cache's actual size.
    order: Vec<ImageCacheKey>,
    entries: HashMap<ImageCacheKey, T>,
}

impl<T> ImageCache<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: Vec::new(),
            entries: HashMap::new(),
        }
    }

    /// Look up a cached decode for `(source, width, height, scale)`. On a
    /// miss, runs `decode` and caches its result if it returns `Some`; a
    /// `None` result is never cached, so a transient decode failure
    /// doesn't wedge a source out of ever being retried on a later paint
    /// (e.g. a file that doesn't exist yet at first paint but is written
    /// before the next one). Marks the entry most-recently-used on both
    /// a hit and a fresh insert.
    pub fn get_or_decode(
        &mut self,
        source: &ImageSource,
        width: u32,
        height: u32,
        scale: f32,
        decode: impl FnOnce() -> Option<T>,
    ) -> Option<&T> {
        let key = ImageCacheKey::new(source, width, height, scale);
        if self.entries.contains_key(&key) {
            self.touch(key);
            return self.entries.get(&key);
        }
        let value = decode()?;
        self.insert(key, value);
        self.entries.get(&key)
    }

    fn touch(&mut self, key: ImageCacheKey) {
        self.order.retain(|k| *k != key);
        self.order.push(key);
    }

    fn insert(&mut self, key: ImageCacheKey, value: T) {
        if self.entries.len() >= self.capacity {
            // Evict the least-recently-used entry first (front of
            // `order`) to make room.
            if !self.order.is_empty() {
                let oldest = self.order.remove(0);
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key, value);
        self.touch(key);
    }
}

impl<T> Default for ImageCache<T> {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn src(bytes: &[u8]) -> ImageSource {
        ImageSource::Bytes(bytes.to_vec())
    }

    #[test]
    fn repeated_lookup_for_same_key_decodes_once() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(4);
        let calls = Cell::new(0);
        let s = src(b"hello");
        for _ in 0..3 {
            let v = cache.get_or_decode(&s, 10, 10, 1.0, || {
                calls.set(calls.get() + 1);
                Some(42)
            });
            assert_eq!(v, Some(&42));
        }
        assert_eq!(calls.get(), 1, "decode should only run on the first miss");
    }

    #[test]
    fn different_size_is_a_distinct_key() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(4);
        let calls = Cell::new(0);
        let s = src(b"hello");
        cache.get_or_decode(&s, 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            Some(1)
        });
        cache.get_or_decode(&s, 20, 20, 1.0, || {
            calls.set(calls.get() + 1);
            Some(2)
        });
        assert_eq!(
            calls.get(),
            2,
            "different target size must not share a cache entry"
        );
    }

    #[test]
    fn different_scale_is_a_distinct_key() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(4);
        let calls = Cell::new(0);
        let s = src(b"hello");
        cache.get_or_decode(&s, 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            Some(1)
        });
        cache.get_or_decode(&s, 10, 10, 2.0, || {
            calls.set(calls.get() + 1);
            Some(2)
        });
        assert_eq!(
            calls.get(),
            2,
            "different scale must not share a cache entry"
        );
    }

    #[test]
    fn different_source_is_a_distinct_key() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(4);
        let calls = Cell::new(0);
        cache.get_or_decode(&src(b"a"), 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            Some(1)
        });
        cache.get_or_decode(&src(b"b"), 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            Some(2)
        });
        assert_eq!(
            calls.get(),
            2,
            "different source bytes must not share a cache entry"
        );
    }

    #[test]
    fn failed_decode_is_not_cached_and_is_retried() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(4);
        let calls = Cell::new(0);
        let s = src(b"hello");
        let v = cache.get_or_decode(&s, 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            None
        });
        assert_eq!(v, None);
        let v = cache.get_or_decode(&s, 10, 10, 1.0, || {
            calls.set(calls.get() + 1);
            Some(7)
        });
        assert_eq!(v, Some(&7));
        assert_eq!(calls.get(), 2, "a failed decode must not be cached");
    }

    #[test]
    fn eviction_drops_least_recently_used_entry_once_over_capacity() {
        let mut cache: ImageCache<u32> = ImageCache::with_capacity(2);
        let calls = Cell::new(0);
        let mut decode_for = |n: u8| {
            let s = src(&[n]);
            cache.get_or_decode(&s, 1, 1, 1.0, || {
                calls.set(calls.get() + 1);
                Some(n as u32)
            });
        };
        decode_for(1);
        decode_for(2);
        // Capacity is 2; both entries fit. Re-decoding either should not
        // increment `calls`.
        let before = calls.get();
        decode_for(1);
        decode_for(2);
        assert_eq!(calls.get(), before, "both entries should still be cached");

        // A third distinct source evicts the least-recently-used one (1,
        // since 2 was touched more recently just above).
        decode_for(3);
        let calls_before_recheck = calls.get();
        decode_for(2); // still cached
        assert_eq!(
            calls.get(),
            calls_before_recheck,
            "2 should still be cached"
        );
        decode_for(1); // evicted, must re-decode
        assert_eq!(
            calls.get(),
            calls_before_recheck + 1,
            "1 should have been evicted"
        );
    }
}
