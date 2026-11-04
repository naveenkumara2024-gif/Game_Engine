//! The asset server (lore §6.8): async loading, handles, sub-asset resolution, hot reload and
//! unloading. Driven from the main thread by [`AssetServer::update`] once per frame; all file
//! IO and importing happens on `ge_tasks` threads.
//!
//! Pipeline per source file:
//! 1. **IO thread:** read the file and its `.meta` (creating one if missing).
//! 2. **Worker:** read dependencies, compute the DDC key, then either load the cached
//!    [`ImportOutput`] or run the importer and store the result.
//! 3. **Main thread:** publish every sub-asset (same ids on reload, version + 1), resolve
//!    references between them, emit events.

use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;

use ge_ecs::prelude::{Resource, bevy_ecs};
use ge_tasks::{Poll, Task, Tasks};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::AssetError;
use crate::ddc::Ddc;
use crate::handle::{AssetId, Handle, SourceToken, UntypedId};
use crate::import::Importer;
use crate::meta::Meta;
use crate::path::AssetPath;
use crate::registry::Registry;
use crate::types::{ImportOutput, MaterialAsset, Mesh, ModelAsset, SubAsset, Texture};
use crate::watch::AssetWatcher;

/// Where an asset is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Unknown or already unloaded id.
    NotLoaded,
    Loading,
    Loaded,
    Failed(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AssetEventKind {
    Loaded,
    Modified,
    Removed,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AssetEvent {
    pub id: UntypedId,
    pub kind: AssetEventKind,
}

enum SlotState<T> {
    Empty,
    Loading,
    Loaded(Arc<T>),
    Failed(Arc<str>),
}

struct Slot<T> {
    generation: u32,
    version: u32,
    state: SlotState<T>,
}

/// Typed storage: a generational slot array.
pub struct Storage<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> std::fmt::Debug for Storage<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("slots", &self.slots.len())
            .field("free", &self.free.len())
            .finish()
    }
}

impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Storage<T> {
    fn alloc(&mut self) -> AssetId<T> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.state = SlotState::Loading;
            slot.version = 0;
            return AssetId::new(index, slot.generation);
        }
        self.slots.push(Slot {
            generation: 0,
            version: 0,
            state: SlotState::Loading,
        });
        AssetId::new(self.slots.len() as u32 - 1, 0)
    }

    fn slot(&self, id: AssetId<T>) -> Option<&Slot<T>> {
        self.slots
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation)
    }

    fn slot_mut(&mut self, id: AssetId<T>) -> Option<&mut Slot<T>> {
        self.slots
            .get_mut(id.index as usize)
            .filter(|s| s.generation == id.generation)
    }

    fn free(&mut self, id: AssetId<T>) {
        if let Some(slot) = self.slot_mut(id) {
            slot.state = SlotState::Empty;
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(id.index);
        }
    }

    pub fn get(&self, id: AssetId<T>) -> Option<&Arc<T>> {
        match &self.slot(id)?.state {
            SlotState::Loaded(v) => Some(v),
            _ => None,
        }
    }

    pub fn version(&self, id: AssetId<T>) -> u32 {
        self.slot(id).map_or(0, |s| s.version)
    }

    fn state(&self, id: AssetId<T>) -> LoadState {
        match self.slot(id).map(|s| &s.state) {
            None | Some(SlotState::Empty) => LoadState::NotLoaded,
            Some(SlotState::Loading) => LoadState::Loading,
            Some(SlotState::Loaded(_)) => LoadState::Loaded,
            Some(SlotState::Failed(e)) => LoadState::Failed(e.to_string()),
        }
    }
}

/// An asset type the server stores.
pub trait Asset: Send + Sync + Sized + 'static {
    const KIND: &'static str;
    fn storage(server: &AssetServer) -> &Storage<Self>;
    fn storage_mut(server: &mut AssetServer) -> &mut Storage<Self>;
}

macro_rules! impl_asset {
    ($t:ty, $field:ident, $kind:literal) => {
        impl Asset for $t {
            const KIND: &'static str = $kind;
            fn storage(server: &AssetServer) -> &Storage<Self> {
                &server.$field
            }
            fn storage_mut(server: &mut AssetServer) -> &mut Storage<Self> {
                &mut server.$field
            }
        }
    };
}
impl_asset!(Mesh, meshes, "Mesh");
impl_asset!(Texture, textures, "Texture");
impl_asset!(MaterialAsset, materials, "Material");
impl_asset!(ModelAsset, models, "Model");

type ImportResult = Result<(ImportOutput, bool, Vec<String>), AssetError>;

enum Stage {
    Io(Task<Result<(Vec<u8>, Meta), AssetError>>),
    Import(Task<ImportResult>),
}

struct Source {
    path: AssetPath,
    importer: Importer,
    token: Weak<SourceToken>,
    labels: HashMap<String, UntypedId>,
    guid: Option<Uuid>,
    /// Files next to a `.gltf` it reads; a change to any of them reloads it.
    dependencies: Vec<String>,
    stage: Option<Stage>,
    reload_queued: bool,
    loaded_once: bool,
    error: Option<String>,
}

/// Counters for HUDs and tests.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetStats {
    pub sources: usize,
    pub in_flight: usize,
    pub imports: u64,
    pub ddc_hits: u64,
    pub ddc_misses: u64,
}

/// The asset server. Lives in the ECS world as a resource (see [`crate::AssetPlugin`]-style
/// wiring in `ge_engine`).
#[derive(Resource)]
pub struct AssetServer {
    root: PathBuf,
    tasks: Tasks,
    ddc: Arc<Ddc>,
    registry: Registry,
    watcher: Option<AssetWatcher>,
    sources: HashMap<AssetPath, Source>,
    meshes: Storage<Mesh>,
    textures: Storage<Texture>,
    materials: Storage<MaterialAsset>,
    models: Storage<ModelAsset>,
    events: Vec<AssetEvent>,
    render_changes: Vec<AssetEvent>,
    imports: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetServer")
            .field("root", &self.root)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl AssetServer {
    /// `GE_ASSET_DIR`, else the workspace `assets/` folder (dev builds run from the source
    /// tree; packaged builds will read cooked containers, M10).
    pub fn default_root() -> PathBuf {
        std::env::var_os("GE_ASSET_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
            PathBuf::from,
        )
    }

    /// `GE_DDC_DIR`, else `<workspace>/.ddc`.
    pub fn default_ddc() -> PathBuf {
        std::env::var_os("GE_DDC_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.ddc"),
            PathBuf::from,
        )
    }

    pub fn new(root: impl Into<PathBuf>, ddc_dir: impl Into<PathBuf>, tasks: Tasks) -> Self {
        let root = root.into();
        if let Err(e) = std::fs::create_dir_all(&root) {
            warn!(root = %root.display(), error = %e, "could not create asset root");
        }
        let registry = Registry::scan(&root);
        Self {
            root,
            tasks,
            ddc: Arc::new(Ddc::new(ddc_dir)),
            registry,
            watcher: None,
            sources: HashMap::new(),
            meshes: Storage::default(),
            textures: Storage::default(),
            materials: Storage::default(),
            models: Storage::default(),
            events: Vec::new(),
            render_changes: Vec::new(),
            imports: Arc::default(),
        }
    }

    /// Starts watching the asset root for hot reload.
    pub fn watch(&mut self) {
        match AssetWatcher::new(&self.root) {
            Ok(w) => {
                info!(root = %self.root.display(), "asset hot reload watching");
                self.watcher = Some(w);
            }
            Err(e) => warn!(error = %e, "asset hot reload unavailable"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Requests `path` (`file.glb` = its main asset, `file.glb#Label` = a sub-asset). Returns
    /// at once; the asset becomes available after a few [`update`](Self::update)s.
    pub fn load<T: Asset>(&mut self, path: impl Into<AssetPath>) -> Handle<T> {
        let path: AssetPath = path.into();
        let source_path = path.source_path();
        let importer = Importer::for_extension(&path.extension());
        let entry = self
            .sources
            .entry(source_path.clone())
            .or_insert_with(|| Source {
                path: source_path.clone(),
                importer: importer.unwrap_or(Importer::Image),
                token: Weak::new(),
                labels: HashMap::new(),
                guid: None,
                dependencies: Vec::new(),
                stage: None,
                reload_queued: false,
                loaded_once: false,
                error: importer
                    .is_none()
                    .then(|| format!("no importer for '.{}' files", path.extension())),
            });
        let token = entry.token.upgrade().unwrap_or_else(|| {
            let t = Arc::new(SourceToken {
                source: source_path.clone(),
            });
            entry.token = Arc::downgrade(&t);
            t
        });
        let label = path
            .label()
            .map_or_else(|| entry.importer.main_label().to_owned(), str::to_owned);
        let existing = entry.labels.get(&label).and_then(|u| u.typed::<T>());
        let failed = entry.error.clone();
        let needs_start = entry.stage.is_none() && !entry.loaded_once && failed.is_none();
        let id = match existing {
            Some(id) => id,
            None => {
                let id = T::storage_mut(self).alloc();
                if let Some(e) = &failed {
                    T::storage_mut(self)
                        .slot_mut(id)
                        .expect("just allocated")
                        .state = SlotState::Failed(e.as_str().into());
                } else if let Some(src) = self.sources.get(&source_path)
                    && src.loaded_once
                {
                    // The source is loaded but has no such label.
                    T::storage_mut(self)
                        .slot_mut(id)
                        .expect("just allocated")
                        .state =
                        SlotState::Failed(format!("'{path}' has no {} '{label}'", T::KIND).into());
                }
                self.sources
                    .get_mut(&source_path)
                    .expect("inserted above")
                    .labels
                    .insert(label, id.untyped());
                id
            }
        };
        if needs_start {
            self.start(&source_path);
        }
        Handle::new(id, token)
    }

    /// Loads by GUID (survives renames). `label` selects a sub-asset (`None` = main asset).
    pub fn load_guid<T: Asset>(&mut self, guid: Uuid, label: Option<&str>) -> Option<Handle<T>> {
        let path = self.registry.path_of(guid)?.clone();
        Some(self.load(match label {
            Some(l) => path.with_label(l),
            None => path,
        }))
    }

    pub fn get<T: Asset>(&self, id: AssetId<T>) -> Option<&Arc<T>> {
        T::storage(self).get(id)
    }

    /// Bumped each time the asset is (re)loaded; 0 until the first load.
    pub fn version<T: Asset>(&self, id: AssetId<T>) -> u32 {
        T::storage(self).version(id)
    }

    pub fn state<T: Asset>(&self, id: AssetId<T>) -> LoadState {
        T::storage(self).state(id)
    }

    /// The GUID of the source file `handle` came from (once its `.meta` was read).
    pub fn guid_of<T>(&self, handle: &Handle<T>) -> Option<Uuid> {
        self.sources.get(handle.source()).and_then(|s| s.guid)
    }

    /// Events from the last [`update`](Self::update) (gameplay-facing).
    pub fn events(&self) -> &[AssetEvent] {
        &self.events
    }

    /// Changes for the renderer, accumulated until drained (by the render extract).
    pub fn drain_render_changes(&mut self) -> Vec<AssetEvent> {
        std::mem::take(&mut self.render_changes)
    }

    pub fn stats(&self) -> AssetStats {
        AssetStats {
            sources: self.sources.len(),
            in_flight: self.sources.values().filter(|s| s.stage.is_some()).count(),
            imports: self.imports.load(std::sync::atomic::Ordering::Relaxed),
            ddc_hits: self.ddc.hits(),
            ddc_misses: self.ddc.misses(),
        }
    }

    /// Re-imports a source (hot reload, or on request).
    pub fn reload(&mut self, source: &AssetPath) {
        let Some(src) = self.sources.get_mut(&source.source_path()) else {
            return;
        };
        if src.stage.is_some() {
            src.reload_queued = true;
        } else {
            let p = src.path.clone();
            self.start(&p);
        }
    }

    fn start(&mut self, source: &AssetPath) {
        let Some(src) = self.sources.get_mut(source) else {
            return;
        };
        let file = source.to_file(&self.root);
        let importer = src.importer;
        debug!(%source, "load started");
        src.stage = Some(Stage::Io(self.tasks.io("Asset::Read", move || {
            let bytes = std::fs::read(&file).map_err(|e| AssetError::io(&file, e))?;
            let (meta, _) = Meta::load_or_create(&file, importer.id(), importer.version())?;
            Ok((bytes, meta))
        })));
    }

    /// Once per frame: advance in-flight loads, apply finished ones, hot reload, unload
    /// sources nobody holds. Never blocks.
    pub fn update(&mut self) {
        let _span = tracing::info_span!("Asset::Update").entered();
        self.events.clear();

        let mut finished: Vec<(AssetPath, ImportResult)> = Vec::new();
        let paths: Vec<AssetPath> = self
            .sources
            .iter()
            .filter(|(_, s)| s.stage.is_some())
            .map(|(p, _)| p.clone())
            .collect();
        for path in paths {
            let src = self.sources.get_mut(&path).expect("listed");
            match src.stage.as_mut().expect("in flight") {
                Stage::Io(task) => match task.poll() {
                    Poll::Pending => {}
                    Poll::Panicked => {
                        finished.push((path, Err(AssetError::Import("read task panicked".into()))))
                    }
                    Poll::Ready(Err(e)) => finished.push((path, Err(e))),
                    Poll::Ready(Ok((bytes, meta))) => {
                        src.guid = Some(meta.guid);
                        let importer = src.importer;
                        let file = path.to_file(&self.root);
                        let ddc = self.ddc.clone();
                        let imports = self.imports.clone();
                        src.stage = Some(Stage::Import(
                            self.tasks.compute("Asset::Import", move || {
                                import_cached(importer, &file, &bytes, &meta, &ddc, &imports)
                            }),
                        ));
                    }
                },
                Stage::Import(task) => match task.poll() {
                    Poll::Pending => {}
                    Poll::Panicked => finished
                        .push((path, Err(AssetError::Import("import task panicked".into())))),
                    Poll::Ready(result) => finished.push((path, result)),
                },
            }
        }
        for (path, result) in finished {
            self.finish(&path, result);
        }

        self.hot_reload();
        self.collect_garbage();
    }

    fn finish(&mut self, path: &AssetPath, result: ImportResult) {
        let Some(src) = self.sources.get_mut(path) else {
            return;
        };
        src.stage = None;
        let reload = src.loaded_once;
        match result {
            Err(e) => {
                warn!(%path, error = %e, "asset failed to load");
                src.error = Some(e.to_string());
                let labels: Vec<UntypedId> = src.labels.values().copied().collect();
                for id in labels {
                    self.set_failed(id, &e.to_string());
                }
            }
            Ok((output, ddc_hit, deps)) => {
                info!(%path, ddc = if ddc_hit { "hit" } else { "miss" }, assets = output.assets.len(), reload, "asset ready");
                src.dependencies = deps;
                src.error = None;
                self.publish(path, output);
                let src = self.sources.get_mut(path).expect("present");
                src.loaded_once = true;
            }
        }
        let src = self.sources.get_mut(path).expect("present");
        if std::mem::take(&mut src.reload_queued) {
            self.start(path);
        }
    }

    fn set_failed(&mut self, id: UntypedId, error: &str) {
        let e: Arc<str> = error.into();
        macro_rules! fail {
            ($t:ty) => {
                if let Some(id) = id.typed::<$t>()
                    && let Some(slot) = <$t>::storage_mut(self).slot_mut(id)
                    && !matches!(slot.state, SlotState::Loaded(_))
                {
                    slot.state = SlotState::Failed(e.clone());
                }
            };
        }
        fail!(Mesh);
        fail!(Texture);
        fail!(MaterialAsset);
        fail!(ModelAsset);
    }

    /// Stores every sub-asset under its label's id (allocating new ones), resolves references,
    /// emits Loaded / Modified events.
    fn publish(&mut self, path: &AssetPath, output: ImportOutput) {
        // Pass 1: ids for every produced label.
        let mut ids: HashMap<String, UntypedId> = HashMap::new();
        for (label, asset) in &output.assets {
            let existing = self.sources[path].labels.get(label).copied();
            let id = match (asset, existing) {
                (SubAsset::Mesh(_), Some(u)) if u.type_id == TypeId::of::<Mesh>() => u,
                (SubAsset::Texture(_), Some(u)) if u.type_id == TypeId::of::<Texture>() => u,
                (SubAsset::Material(_), Some(u)) if u.type_id == TypeId::of::<MaterialAsset>() => u,
                (SubAsset::Model(_), Some(u)) if u.type_id == TypeId::of::<ModelAsset>() => u,
                (SubAsset::Mesh(_), _) => self.meshes.alloc().untyped(),
                (SubAsset::Texture(_), _) => self.textures.alloc().untyped(),
                (SubAsset::Material(_), _) => self.materials.alloc().untyped(),
                (SubAsset::Model(_), _) => self.models.alloc().untyped(),
            };
            ids.insert(label.clone(), id);
        }
        // Pass 2: resolve references and store.
        for (label, asset) in output.assets {
            let id = ids[&label];
            match asset {
                SubAsset::Mesh(m) => self.store(id.typed::<Mesh>().expect("typed"), m),
                SubAsset::Texture(t) => self.store(id.typed::<Texture>().expect("typed"), t),
                SubAsset::Material(mut m) => {
                    for r in m.textures_mut() {
                        r.id = ids.get(&r.label).and_then(|u| u.typed());
                    }
                    self.store(id.typed::<MaterialAsset>().expect("typed"), m);
                }
                SubAsset::Model(mut model) => {
                    for node in &mut model.nodes {
                        for p in &mut node.primitives {
                            p.mesh.id = ids.get(&p.mesh.label).and_then(|u| u.typed());
                            if let Some(mat) = &mut p.material {
                                mat.id = ids.get(&mat.label).and_then(|u| u.typed());
                            }
                        }
                    }
                    self.store(id.typed::<ModelAsset>().expect("typed"), model);
                }
            }
        }
        // Labels that were requested but not produced.
        let src = self.sources.get_mut(path).expect("present");
        let missing: Vec<(String, UntypedId)> = src
            .labels
            .iter()
            .filter(|(l, _)| !ids.contains_key(*l))
            .map(|(l, u)| (l.clone(), *u))
            .collect();
        src.labels.extend(ids);
        for (label, id) in missing {
            self.set_failed(id, &format!("'{path}' has no sub-asset '{label}'"));
        }
    }

    fn store<T: Asset>(&mut self, id: AssetId<T>, value: T) {
        let Some(slot) = T::storage_mut(self).slot_mut(id) else {
            return;
        };
        let kind = if matches!(slot.state, SlotState::Loaded(_)) {
            AssetEventKind::Modified
        } else {
            AssetEventKind::Loaded
        };
        slot.state = SlotState::Loaded(Arc::new(value));
        slot.version += 1;
        let event = AssetEvent {
            id: id.untyped(),
            kind,
        };
        self.events.push(event);
        self.render_changes.push(event);
    }

    fn hot_reload(&mut self) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        let changed = watcher.poll(Instant::now());
        if changed.is_empty() {
            return;
        }
        let root = self.root.clone();
        let rel: Vec<AssetPath> = changed
            .iter()
            .filter_map(|p| AssetPath::from_file(&root, p))
            .collect();
        let touches_registry = rel
            .iter()
            .any(|p| Importer::for_extension(&p.extension()).is_some());
        if touches_registry {
            self.registry = Registry::scan(&root);
        }
        let to_reload: Vec<AssetPath> = self
            .sources
            .values()
            .filter(|s| {
                rel.iter().any(|c| {
                    let c = c.source();
                    c == s.path.source()
                        || c.strip_suffix(".meta") == Some(s.path.source())
                        || s.dependencies.iter().any(|d| {
                            let dir = s.path.source().rsplit_once('/').map_or("", |(d, _)| d);
                            let dep = if dir.is_empty() {
                                d.clone()
                            } else {
                                format!("{dir}/{d}")
                            };
                            dep == c
                        })
                })
            })
            .map(|s| s.path.clone())
            .collect();
        for path in to_reload {
            info!(%path, "source changed on disk; reloading");
            self.reload(&path);
        }
    }

    /// Unloads sources whose last strong handle dropped (and that aren't mid-load).
    fn collect_garbage(&mut self) {
        let dead: Vec<AssetPath> = self
            .sources
            .iter()
            .filter(|(_, s)| s.token.strong_count() == 0 && s.stage.is_none())
            .map(|(p, _)| p.clone())
            .collect();
        for path in dead {
            let src = self.sources.remove(&path).expect("listed");
            debug!(%path, "unloading");
            for id in src.labels.values() {
                macro_rules! free {
                    ($t:ty) => {
                        if let Some(typed) = id.typed::<$t>() {
                            <$t>::storage_mut(self).free(typed);
                        }
                    };
                }
                free!(Mesh);
                free!(Texture);
                free!(MaterialAsset);
                free!(ModelAsset);
                let event = AssetEvent {
                    id: *id,
                    kind: AssetEventKind::Removed,
                };
                self.events.push(event);
                self.render_changes.push(event);
            }
        }
    }
}

/// Worker-side: dependencies, DDC lookup, import, DDC store.
fn import_cached(
    importer: Importer,
    file: &Path,
    bytes: &[u8],
    meta: &Meta,
    ddc: &Ddc,
    imports: &std::sync::atomic::AtomicU64,
) -> ImportResult {
    let deps: BTreeMap<String, Vec<u8>> = importer.read_dependencies(file, bytes)?;
    let dep_slices: Vec<&[u8]> = deps.values().map(Vec::as_slice).collect();
    let key = Ddc::key(
        importer.id(),
        importer.version(),
        &meta.settings,
        bytes,
        &dep_slices,
    );
    let names: Vec<String> = deps.keys().cloned().collect();
    if let Some(cached) = ddc.get(&key) {
        return Ok((cached, true, names));
    }
    let output = importer.import(bytes, &deps, &meta.settings)?;
    imports.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ddc.put(&key, &output);
    Ok((output, false, names))
}
