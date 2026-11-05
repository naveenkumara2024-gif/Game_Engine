//! Derived-data cache (lore §3.8, §6.8; plan 0005 decision 6). Imported results are stored by
//! a key that covers *everything* that could change them:
//! `blake3(DDC format ‖ importer id ‖ importer version ‖ settings ‖ source bytes ‖ deps)`.
//! A hit skips the import entirely. Entries are postcard-encoded [`ImportOutput`]s, written
//! atomically; a corrupt entry is deleted and treated as a miss.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::warn;

use crate::meta::{ImportSettings, write_atomic};
use crate::types::ImportOutput;

/// Bump when the payload layout (`types.rs`) changes: every old entry then misses.
pub const DDC_FORMAT_VERSION: u32 = 2;

#[derive(Debug)]
pub struct Ddc {
    dir: PathBuf,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// A 64-hex-digit blake3 key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DdcKey(pub String);

impl Ddc {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn key(
        importer: &str,
        importer_version: u32,
        settings: &ImportSettings,
        source: &[u8],
        dependencies: &[&[u8]],
    ) -> DdcKey {
        let mut h = blake3::Hasher::new();
        h.update(&DDC_FORMAT_VERSION.to_le_bytes());
        h.update(importer.as_bytes());
        h.update(&[0]);
        h.update(&importer_version.to_le_bytes());
        h.update(ron::to_string(settings).unwrap_or_default().as_bytes());
        h.update(&(source.len() as u64).to_le_bytes());
        h.update(source);
        for dep in dependencies {
            h.update(&(dep.len() as u64).to_le_bytes());
            h.update(dep);
        }
        DdcKey(h.finalize().to_hex().to_string())
    }

    fn file(&self, key: &DdcKey) -> PathBuf {
        // Two-level fan-out keeps directories small.
        self.dir.join(&key.0[..2]).join(&key.0)
    }

    pub fn get(&self, key: &DdcKey) -> Option<ImportOutput> {
        let path = self.file(key);
        let Ok(bytes) = std::fs::read(&path) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        match postcard::from_bytes::<ImportOutput>(&bytes) {
            Ok(out) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(out)
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "corrupt DDC entry; re-importing");
                let _ = std::fs::remove_file(&path);
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn put(&self, key: &DdcKey, output: &ImportOutput) {
        let path = self.file(key);
        let result = postcard::to_allocvec(output)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                write_atomic(&path, &bytes).map_err(|e| e.to_string())
            });
        if let Err(e) = result {
            warn!(path = %path.display(), error = %e, "could not write DDC entry");
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Mesh, SubAsset};

    #[test]
    fn key_changes_with_every_input() {
        let s = ImportSettings::default();
        let base = Ddc::key("gltf", 1, &s, b"abc", &[b"dep"]);
        assert_eq!(
            base,
            Ddc::key("gltf", 1, &s, b"abc", &[b"dep"]),
            "deterministic"
        );
        assert_ne!(
            base,
            Ddc::key("gltf", 2, &s, b"abc", &[b"dep"]),
            "importer version"
        );
        assert_ne!(
            base,
            Ddc::key("image", 1, &s, b"abc", &[b"dep"]),
            "importer"
        );
        assert_ne!(base, Ddc::key("gltf", 1, &s, b"abd", &[b"dep"]), "source");
        assert_ne!(
            base,
            Ddc::key("gltf", 1, &s, b"abc", &[b"deq"]),
            "dependency"
        );
        let scaled = ImportSettings {
            scale: 2.0,
            ..Default::default()
        };
        assert_ne!(
            base,
            Ddc::key("gltf", 1, &scaled, b"abc", &[b"dep"]),
            "settings"
        );
        // Moving bytes between source and dependency must not collide.
        assert_ne!(
            Ddc::key("gltf", 1, &s, b"ab", &[b"c"]),
            Ddc::key("gltf", 1, &s, b"a", &[b"bc"])
        );
    }

    #[test]
    fn hit_miss_and_corruption() {
        let dir = std::env::temp_dir().join(format!("ge_ddc_{}", std::process::id()));
        let ddc = Ddc::new(&dir);
        let key = Ddc::key("t", 1, &ImportSettings::default(), b"x", &[]);
        assert!(ddc.get(&key).is_none());
        let out = ImportOutput {
            assets: vec![("Mesh0".into(), SubAsset::Mesh(Mesh::default()))],
        };
        ddc.put(&key, &out);
        assert_eq!(ddc.get(&key), Some(out));
        assert_eq!((ddc.hits(), ddc.misses()), (1, 1));
        std::fs::write(ddc.file(&key), b"\xff\xff\xff garbage").unwrap();
        assert!(ddc.get(&key).is_none(), "corrupt entry is a miss");
        assert!(!ddc.file(&key).exists(), "and is deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
