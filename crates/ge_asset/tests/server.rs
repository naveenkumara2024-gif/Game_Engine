//! AssetServer end to end, on a temporary asset root and DDC (plan 0005 acceptance criteria).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ge_asset::fixtures::{checker_bc1_ktx2, crate_glb};
use ge_asset::{
    AssetEventKind, AssetServer, LoadState, MaterialAsset, Mesh, ModelAsset, PixelFormat, Texture,
};
use ge_tasks::Tasks;

struct Dirs {
    root: PathBuf,
    ddc: PathBuf,
}

impl Dirs {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("ge_asset_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("assets");
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::write(root.join("models/crate.glb"), crate_glb()).unwrap();
        std::fs::write(root.join("checker_bc1.ktx2"), checker_bc1_ktx2()).unwrap();
        Self {
            root,
            ddc: base.join("ddc"),
        }
    }

    fn server(&self) -> AssetServer {
        AssetServer::new(&self.root, &self.ddc, Tasks::new(2, Some(2)))
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        if let Some(base) = self.root.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
    }
}

/// Polls the server until `done` holds (or fails after 10 s).
fn pump(server: &mut AssetServer, mut done: impl FnMut(&AssetServer) -> bool) {
    let start = Instant::now();
    while !done(server) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {server:?}"
        );
        server.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn glb_loads_async_with_sub_assets_resolved() {
    let dirs = Dirs::new("glb");
    let mut server = dirs.server();
    assert_eq!(
        server.registry().models().count(),
        1,
        "registry found the model"
    );

    let started = Instant::now();
    let model = server.load::<ModelAsset>("models/crate.glb");
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "load() must not block"
    );
    assert_eq!(server.state(model.id()), LoadState::Loading);
    pump(&mut server, |s| s.state(model.id()) == LoadState::Loaded);

    let m = server.get(model.id()).unwrap().clone();
    assert_eq!(m.roots, [0]);
    assert_eq!(m.nodes.len(), 2);
    assert_eq!(m.nodes[0].name, "Crate");
    assert_eq!(m.nodes[0].children, [1]);
    assert_eq!(m.nodes[1].scale, [1.04, 0.12, 1.04]);
    let wood_prim = &m.nodes[0].primitives[0];
    let mesh = server
        .get(wood_prim.mesh.id.expect("mesh resolved"))
        .unwrap();
    assert_eq!((mesh.vertex_count(), mesh.triangle_count()), (24, 12));
    assert_eq!(mesh.bounds_max, [0.5; 3]);

    let wood = server
        .get(
            wood_prim
                .material
                .as_ref()
                .unwrap()
                .id
                .expect("material resolved"),
        )
        .unwrap()
        .clone();
    assert_eq!(wood.name, "Wood");
    assert_eq!((wood.metallic, wood.roughness), (0.0, 0.7));
    let tex = server
        .get(
            wood.base_color_texture
                .as_ref()
                .unwrap()
                .id
                .expect("texture resolved"),
        )
        .unwrap();
    assert_eq!(
        (tex.width, tex.height, tex.format),
        (64, 64, PixelFormat::Rgba8Srgb)
    );
    assert_eq!(tex.mips.len(), 7, "64 → 1");

    let metal_id = m.nodes[1].primitives[0]
        .material
        .as_ref()
        .unwrap()
        .id
        .unwrap();
    let metal: &MaterialAsset = server.get(metal_id).unwrap();
    assert_eq!(metal.metallic, 1.0);
    assert!(metal.base_color_texture.is_none());

    // Sub-assets by label: same ids as the resolved references.
    let mesh_handle = server.load::<Mesh>("models/crate.glb#Mesh0/0");
    assert_eq!(
        Some(mesh_handle.id()),
        wood_prim.mesh.id,
        "same id for the same label"
    );
    let missing = server.load::<Mesh>("models/crate.glb#Mesh9/9");
    assert!(matches!(server.state(missing.id()), LoadState::Failed(_)));
    assert!(server.guid_of(&model).is_some());
}

#[test]
fn ktx2_bc1_loads_natively() {
    let dirs = Dirs::new("ktx2");
    let mut server = dirs.server();
    let tex = server.load::<Texture>("checker_bc1.ktx2");
    pump(&mut server, |s| s.state(tex.id()) == LoadState::Loaded);
    let t = server.get(tex.id()).unwrap();
    assert_eq!(
        (t.format, t.width, t.height, t.mips.len()),
        (PixelFormat::Bc1Srgb, 64, 64, 7)
    );
}

#[test]
fn second_load_hits_the_ddc_and_settings_change_misses() {
    let dirs = Dirs::new("ddc");
    {
        let mut server = dirs.server();
        let h = server.load::<ModelAsset>("models/crate.glb");
        pump(&mut server, |s| s.state(h.id()) == LoadState::Loaded);
        assert_eq!(server.stats().imports, 1);
    }
    {
        let mut server = dirs.server();
        let h = server.load::<ModelAsset>("models/crate.glb");
        pump(&mut server, |s| s.state(h.id()) == LoadState::Loaded);
        let stats = server.stats();
        assert_eq!((stats.imports, stats.ddc_hits), (0, 1), "{stats:?}");
    }
    // Change an import setting in the .meta: the key changes, so it re-imports.
    let meta_path = dirs.root.join("models/crate.glb.meta");
    let text = std::fs::read_to_string(&meta_path).unwrap();
    std::fs::write(&meta_path, text.replace("scale: 1.0", "scale: 2.0")).unwrap();
    let mut server = dirs.server();
    let h = server.load::<ModelAsset>("models/crate.glb");
    pump(&mut server, |s| s.state(h.id()) == LoadState::Loaded);
    assert_eq!(server.stats().imports, 1, "settings change → re-import");
    let model = server.get(h.id()).unwrap();
    assert_eq!(model.nodes[0].scale, [2.0; 3]);
}

#[test]
fn reload_keeps_ids_and_bumps_versions() {
    let dirs = Dirs::new("reload");
    let mut server = dirs.server();
    let tex = server.load::<Texture>("checker_bc1.ktx2");
    pump(&mut server, |s| s.state(tex.id()) == LoadState::Loaded);
    assert_eq!(server.version(tex.id()), 1);
    let _ = server.drain_render_changes();

    // A different texture under the same name.
    std::fs::write(
        dirs.root.join("checker_bc1.ktx2"),
        ge_asset::import::ktx2::write(PixelFormat::Rgba8, 1, 1, &[vec![1, 2, 3, 4]]),
    )
    .unwrap();
    server.reload(tex.source());
    pump(&mut server, |s| s.version(tex.id()) == 2);
    let t = server.get(tex.id()).unwrap();
    assert_eq!((t.format, t.width), (PixelFormat::Rgba8, 1));
    let changes = server.drain_render_changes();
    assert!(
        changes
            .iter()
            .any(|e| e.kind == AssetEventKind::Modified && e.id == tex.id().untyped()),
        "{changes:?}"
    );
}

#[test]
fn dropping_the_last_handle_unloads() {
    let dirs = Dirs::new("gc");
    let mut server = dirs.server();
    let model = server.load::<ModelAsset>("models/crate.glb");
    pump(&mut server, |s| s.state(model.id()) == LoadState::Loaded);
    let id = model.id();
    let clone = model.clone();
    drop(model);
    server.update();
    assert_eq!(
        server.state(id),
        LoadState::Loaded,
        "a clone still holds it"
    );
    drop(clone);
    server.update();
    assert_eq!(server.state(id), LoadState::NotLoaded);
    assert!(server.get(id).is_none(), "stale id resolves to None");
    assert_eq!(server.stats().sources, 0);
}

#[test]
fn hot_reload_from_the_file_watcher() {
    let dirs = Dirs::new("watch");
    let mut server = dirs.server();
    server.watch();
    let tex = server.load::<Texture>("checker_bc1.ktx2");
    pump(&mut server, |s| s.state(tex.id()) == LoadState::Loaded);
    std::thread::sleep(Duration::from_millis(300)); // let the watcher settle (meta writes)
    for _ in 0..20 {
        server.update();
    }
    let before = server.version(tex.id());
    write_ktx2(&dirs.root, 2);
    pump(&mut server, |s| s.version(tex.id()) > before);
    assert_eq!(server.get(tex.id()).unwrap().width, 2);
}

fn write_ktx2(root: &Path, size: u32) {
    let px = (size * size * 4) as usize;
    std::fs::write(
        root.join("checker_bc1.ktx2"),
        ge_asset::import::ktx2::write(PixelFormat::Rgba8, size, size, &[vec![9; px]]),
    )
    .unwrap();
}

#[test]
fn unknown_and_missing_files_fail_cleanly() {
    let dirs = Dirs::new("fail");
    let mut server = dirs.server();
    let missing = server.load::<ModelAsset>("models/nope.glb");
    pump(&mut server, |s| {
        matches!(s.state(missing.id()), LoadState::Failed(_))
    });
    let odd = server.load::<Texture>("readme.txt");
    assert!(matches!(server.state(odd.id()), LoadState::Failed(_)));
}
