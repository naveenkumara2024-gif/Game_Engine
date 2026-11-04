//! The asset registry (lore §6.8): every importable file under the asset root, with its GUID
//! (from the `.meta` sidecar, created on first scan) and importer. Feeds GUID lookups, the
//! editor's Add ▸ Model list and (later) the asset browser and cooker.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use tracing::{info, warn};
use uuid::Uuid;

use crate::import::Importer;
use crate::meta::Meta;
use crate::path::AssetPath;

#[derive(Clone, Debug, PartialEq)]
pub struct RegistryEntry {
    pub path: AssetPath,
    pub guid: Uuid,
    pub importer: Importer,
}

#[derive(Debug, Default)]
pub struct Registry {
    by_path: BTreeMap<AssetPath, RegistryEntry>,
    by_guid: HashMap<Uuid, AssetPath>,
}

impl Registry {
    /// Walks `root`, creating missing `.meta` files. Unreadable directories are skipped.
    pub fn scan(root: &Path) -> Self {
        let mut reg = Self::default();
        let mut created = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file = entry.path();
                if file.is_dir() {
                    stack.push(file);
                    continue;
                }
                let Some(path) = AssetPath::from_file(root, &file) else {
                    continue;
                };
                let Some(importer) = Importer::for_extension(&path.extension()) else {
                    continue;
                };
                match Meta::load_or_create(&file, importer.id(), importer.version()) {
                    Ok((meta, was_created)) => {
                        created += usize::from(was_created);
                        if let Some(other) = reg.by_guid.get(&meta.guid) {
                            warn!(%path, %other, guid = %meta.guid, "two assets share a GUID (copied .meta?); keeping the first");
                            continue;
                        }
                        reg.by_guid.insert(meta.guid, path.clone());
                        reg.by_path.insert(
                            path.clone(),
                            RegistryEntry {
                                path,
                                guid: meta.guid,
                                importer,
                            },
                        );
                    }
                    Err(e) => warn!(%path, error = %e, "skipping asset with a broken .meta"),
                }
            }
        }
        info!(assets = reg.by_path.len(), new_meta = created, root = %root.display(), "asset registry scanned");
        reg
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    pub fn get(&self, path: &AssetPath) -> Option<&RegistryEntry> {
        self.by_path.get(&path.source_path())
    }

    pub fn path_of(&self, guid: Uuid) -> Option<&AssetPath> {
        self.by_guid.get(&guid)
    }

    pub fn entries(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.by_path.values()
    }

    /// Every model (glTF) source, sorted by path.
    pub fn models(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries().filter(|e| e.importer == Importer::Gltf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_creates_meta_and_resolves_both_ways() {
        let root = std::env::temp_dir().join(format!("ge_registry_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::write(root.join("models/crate.glb"), b"glb").unwrap();
        std::fs::write(root.join("notes.txt"), b"ignored").unwrap();
        let reg = Registry::scan(&root);
        assert_eq!(reg.len(), 1);
        let entry = reg
            .get(&AssetPath::parse("models/crate.glb#Mesh0/0"))
            .unwrap();
        assert_eq!(entry.importer, Importer::Gltf);
        assert_eq!(
            reg.path_of(entry.guid).unwrap().source(),
            "models/crate.glb"
        );
        assert!(root.join("models/crate.glb.meta").exists());
        // Rescan keeps the GUID.
        let again = Registry::scan(&root);
        assert_eq!(again.get(&entry.path).unwrap().guid, entry.guid);
        assert_eq!(again.models().count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
