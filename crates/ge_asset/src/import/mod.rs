//! Importers: source bytes (+ dependencies) → [`ImportOutput`]. Pure functions of their
//! inputs, so their results can be cached in the DDC and they can run on any worker thread.

pub mod gltf;
pub mod image;
pub mod ktx2;

use std::collections::BTreeMap;
use std::path::Path;

use crate::AssetError;
use crate::meta::ImportSettings;
use crate::types::{ImportOutput, SubAsset};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Importer {
    Gltf,
    Image,
    Ktx2,
}

impl Importer {
    pub fn for_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "glb" | "gltf" => Self::Gltf,
            "png" | "jpg" | "jpeg" => Self::Image,
            "ktx2" => Self::Ktx2,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Gltf => "gltf",
            Self::Image => "image",
            Self::Ktx2 => "ktx2",
        }
    }

    /// Bump when an importer's output changes for the same input: the DDC then re-imports.
    pub fn version(self) -> u32 {
        match self {
            Self::Gltf => gltf::IMPORTER_VERSION,
            Self::Image | Self::Ktx2 => 1,
        }
    }

    /// The label of the asset a plain path (no `#label`) refers to.
    pub fn main_label(self) -> &'static str {
        match self {
            Self::Gltf => "Model",
            Self::Image | Self::Ktx2 => "Texture",
        }
    }

    /// Reads the files this source depends on (glTF buffers/images next to a `.gltf`).
    pub fn read_dependencies(
        self,
        source_file: &Path,
        bytes: &[u8],
    ) -> Result<BTreeMap<String, Vec<u8>>, AssetError> {
        let mut deps = BTreeMap::new();
        if self == Self::Gltf {
            let dir = source_file.parent().unwrap_or(Path::new("."));
            for uri in gltf::dependencies(bytes)? {
                let file = dir.join(&uri);
                let data = std::fs::read(&file).map_err(|e| AssetError::io(&file, e))?;
                deps.insert(uri, data);
            }
        }
        Ok(deps)
    }

    pub fn import(
        self,
        bytes: &[u8],
        deps: &BTreeMap<String, Vec<u8>>,
        settings: &ImportSettings,
    ) -> Result<ImportOutput, AssetError> {
        let _span = tracing::info_span!("Asset::Import", importer = self.id()).entered();
        match self {
            Self::Gltf => gltf::import(bytes, deps, settings),
            Self::Image => {
                let srgb = settings.srgb.unwrap_or(true);
                let t = image::decode(bytes, srgb, settings.generate_mips)?;
                Ok(ImportOutput {
                    assets: vec![("Texture".into(), SubAsset::Texture(t))],
                })
            }
            Self::Ktx2 => Ok(ImportOutput {
                assets: vec![("Texture".into(), SubAsset::Texture(ktx2::import(bytes)?))],
            }),
        }
    }
}
