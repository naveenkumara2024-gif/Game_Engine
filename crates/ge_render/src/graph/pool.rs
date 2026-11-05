//! Cross-frame pool of physical transient textures. Keys are `(desc, ordinal)`: the k-th
//! texture with that descriptor the compiled graph needs. The same graph shape therefore gets
//! the same textures every frame, so frames don't allocate in steady state. A texture unused
//! for [`MAX_IDLE_FRAMES`] frames is freed. That covers resizes: old sizes simply age out.

use std::collections::HashMap;

use super::TransientDesc;

pub(crate) const MAX_IDLE_FRAMES: u64 = 3;

#[derive(Debug)]
pub(crate) struct TransientPool<T> {
    entries: HashMap<(TransientDesc, u32), (T, u64)>,
    frame: u64,
}

impl<T> Default for TransientPool<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
        }
    }
}

impl<T> TransientPool<T> {
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The texture for `key`, created on first use.
    pub(crate) fn acquire(&mut self, key: (TransientDesc, u32), create: impl FnOnce() -> T) -> &T {
        let frame = self.frame;
        let entry = self.entries.entry(key).or_insert_with(|| (create(), frame));
        entry.1 = frame;
        &entry.0
    }

    /// Frees textures no frame has used for a while. Returns how many were freed.
    pub(crate) fn evict_idle(&mut self) -> usize {
        let before = self.entries.len();
        let frame = self.frame;
        self.entries
            .retain(|_, (_, last)| frame - *last <= MAX_IDLE_FRAMES);
        before - self.entries.len()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ge_rhi::TextureFormat;

    fn key(w: u32, ordinal: u32) -> (TransientDesc, u32) {
        (
            TransientDesc::target((w, w), TextureFormat::Rgba16Float),
            ordinal,
        )
    }

    #[test]
    fn steady_state_reuses_and_idle_entries_age_out() {
        let mut pool = TransientPool::default();
        let mut created = 0;
        for _ in 0..5 {
            pool.begin_frame();
            pool.acquire(key(8, 0), || {
                created += 1;
                created
            });
            pool.evict_idle();
        }
        assert_eq!(created, 1, "same key every frame → one texture");

        // "Resize": a new size takes over, the old entry is freed after MAX_IDLE_FRAMES.
        let mut freed = 0;
        for _ in 0..=MAX_IDLE_FRAMES {
            pool.begin_frame();
            pool.acquire(key(16, 0), || 99);
            freed += pool.evict_idle();
        }
        assert_eq!(freed, 1);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn ordinals_are_distinct_textures() {
        let mut pool = TransientPool::default();
        pool.begin_frame();
        let a = *pool.acquire(key(8, 0), || 1);
        let b = *pool.acquire(key(8, 1), || 2);
        assert_ne!(a, b);
    }
}
