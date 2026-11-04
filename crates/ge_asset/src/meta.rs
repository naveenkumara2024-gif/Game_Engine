//! `.meta` sidecars (lore §6.8, plan 0005 decision 5): `crate.glb.meta` next to `crate.glb`,
//! RON with a schema version. Holds the asset's **GUID** (stable across renames when the
//! `.meta` moves with its source) and its import settings. Created on first scan; committed to
//! version control.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AssetError;

pub const META_VERSION: u32 = 1;

/// Import settings. Every field has a default so old `.meta` files keep loading as fields are
/// added (schema migrations come with the version bump).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    /// Uniform scale applied to positions (models).
    pub scale: f32,
    /// Flip V (some exporters disagree with glTF's top-left origin).
    pub flip_uvs: bool,
    /// Build a full mip chain for decoded images.
    pub generate_mips: bool,
    /// Force a standalone image to be color (sRGB) or data (linear). `None` = color.
    pub srgb: Option<bool>,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            flip_uvs: false,
            generate_mips: true,
            srgb: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub version: u32,
    pub guid: Uuid,
    pub importer: String,
    pub importer_version: u32,
    #[serde(default)]
    pub settings: ImportSettings,
}

impl Meta {
    pub fn new(importer: &str, importer_version: u32) -> Self {
        Self {
            version: META_VERSION,
            guid: Uuid::new_v4(),
            importer: importer.to_owned(),
            importer_version,
            settings: ImportSettings::default(),
        }
    }

    pub fn path_for(source: &Path) -> PathBuf {
        let mut s = source.as_os_str().to_owned();
        s.push(".meta");
        PathBuf::from(s)
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::new().depth_limit(3);
        ron::ser::to_string_pretty(self, config).unwrap_or_default()
    }

    pub fn from_ron(text: &str) -> Result<Self, AssetError> {
        ron::from_str(text).map_err(|e| AssetError::Meta(e.to_string()))
    }

    /// Reads the sidecar of `source`, or creates (and writes) one. Returns `(meta, created)`.
    /// A sidecar from another importer keeps its GUID but switches importer.
    pub fn load_or_create(
        source: &Path,
        importer: &str,
        importer_version: u32,
    ) -> Result<(Self, bool), AssetError> {
        let path = Self::path_for(source);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut meta = Self::from_ron(&text)
                .map_err(|e| AssetError::Meta(format!("{}: {e}", path.display())))?;
            if meta.importer != importer {
                meta.importer = importer.to_owned();
                meta.importer_version = importer_version;
                write_atomic(&path, meta.to_ron().as_bytes())?;
            }
            return Ok((meta, false));
        }
        let meta = Self::new(importer, importer_version);
        write_atomic(&path, meta.to_ron().as_bytes())?;
        Ok((meta, true))
    }
}

/// Writes via a temporary file + rename, so readers never see a half-written file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let tmp = path.with_extension(format!(
        "{}.tmp{}",
        path.extension()
            .map(|e| e.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| AssetError::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AssetError::io(path, e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ron_round_trip_and_defaults_for_missing_fields() {
        let meta = Meta::new("gltf", 1);
        let back = Meta::from_ron(&meta.to_ron()).unwrap();
        assert_eq!(back, meta);
        assert_eq!(meta.guid.get_version_num(), 4);
        let old = format!(
            "(version: 1, guid: \"{}\", importer: \"image\", importer_version: 1)",
            meta.guid
        );
        let parsed = Meta::from_ron(&old).unwrap();
        assert_eq!(parsed.settings, ImportSettings::default());
    }

    #[test]
    fn created_once_then_reused_and_moves_with_its_source() {
        let dir = std::env::temp_dir().join(format!("ge_meta_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.png");
        std::fs::write(&src, b"x").unwrap();
        let (first, created) = Meta::load_or_create(&src, "image", 1).unwrap();
        assert!(created);
        let (again, created) = Meta::load_or_create(&src, "image", 1).unwrap();
        assert!(!created);
        assert_eq!(again.guid, first.guid);
        // Rename source + sidecar together: same GUID.
        let moved = dir.join("b.png");
        std::fs::rename(&src, &moved).unwrap();
        std::fs::rename(Meta::path_for(&src), Meta::path_for(&moved)).unwrap();
        let (after, created) = Meta::load_or_create(&moved, "image", 1).unwrap();
        assert!(!created);
        assert_eq!(after.guid, first.guid);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
