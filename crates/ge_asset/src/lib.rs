//! Assets (lore §2.9, §3.8, §6.8; plan 0005). Three stores:
//!
//! 1. **Source:** files under the asset root (`assets/`) with `.meta` sidecars holding a GUID
//!    and import settings ([`meta`], [`registry`]).
//! 2. **DDC:** imported results cached by a content hash of everything that affects them
//!    ([`ddc`]).
//! 3. **Cooked containers:** TOC + chunk store, built by the cooker at M10.
//!
//! Gameplay and the renderer only ever hold [`Handle`]s / [`AssetId`]s. Loading is
//! asynchronous ([`AssetServer::load`] returns at once) and hot reload swaps an asset's data
//! under the same id with a new version.

#![forbid(unsafe_code)]

pub mod ddc;
pub mod fixtures;
mod handle;
pub mod import;
pub mod meta;
mod path;
pub mod registry;
mod server;
pub mod types;
mod watch;

use std::path::Path;

pub use handle::{AssetId, Handle, UntypedId};
pub use import::Importer;
pub use meta::{ImportSettings, Meta};
pub use path::AssetPath;
pub use registry::{Registry, RegistryEntry};
pub use server::{Asset, AssetEvent, AssetEventKind, AssetServer, AssetStats, LoadState, Storage};
pub use types::{
    AlphaMode, ImportOutput, MaterialAsset, Mesh, ModelAsset, ModelNode, ModelPrimitive,
    PixelFormat, SubAsset, SubRef, Texture,
};
pub use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error("bad .meta: {0}")]
    Meta(String),
    #[error("import failed: {0}")]
    Import(String),
}

impl AssetError {
    pub fn io(path: &Path, e: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        }
    }
}
