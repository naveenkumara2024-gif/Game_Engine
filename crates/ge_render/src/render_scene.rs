//! Render world (plan 0008 step 2; lore §3.7 proxies + cached mesh draw commands, §6.7 GPU
//! scene). The game side sends a [`SceneDelta`]: which proxies changed since the last frame
//! that was rendered (Unreal's `MarkRenderStateDirty`, done with ECS change detection). The
//! render side keeps a persistent [`RenderScene`]:
//!
//! - **proxies** live in fixed slots of the object buffer, so a moved object rewrites only
//!   its own slot (contiguous dirty slots go up in one write);
//! - **draw bins** (one per pass) hold [`DrawPacket`]s sorted by pipeline variant, then
//!   material, so passes switch state as little as possible. Bins are rebuilt only when a
//!   proxy is added or removed, or changes something a bin depends on. Moving doesn't count;
//!   transparents are re-sorted by distance every frame, since the camera moves.

use std::collections::HashMap;

use ge_asset::{AssetId, MaterialAsset};
use tracing::warn;

use crate::{DrawAlpha, DrawItem, MAX_OBJECTS, MeshKind};

/// Identifies a proxy across frames (the game side uses `Entity::to_bits`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProxyKey(pub u64);

/// Proxy changes waiting for the renderer. Later changes to a key replace earlier ones, so
/// a delta that isn't drained (minimized window, headless app) stays as large as the scene,
/// not the number of frames.
#[derive(Clone, Debug, Default)]
pub struct SceneDelta {
    pending: HashMap<ProxyKey, Option<DrawItem>>,
}

impl SceneDelta {
    pub fn upsert(&mut self, key: ProxyKey, item: DrawItem) {
        self.pending.insert(key, Some(item));
    }

    pub fn remove(&mut self, key: ProxyKey) {
        self.pending.insert(key, None);
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Pending changes (upserts + removes).
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// The pending upserts (any order).
    pub fn upserts(&self) -> impl Iterator<Item = (ProxyKey, &DrawItem)> {
        self.pending
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|item| (*k, item)))
    }

    /// The pending removes.
    pub fn removes(&self) -> impl Iterator<Item = ProxyKey> + '_ {
        self.pending
            .iter()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| *k)
    }

    pub fn get(&self, key: ProxyKey) -> Option<Option<&DrawItem>> {
        self.pending.get(&key).map(Option::as_ref)
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = (ProxyKey, Option<DrawItem>)> + '_ {
        self.pending.drain()
    }
}

/// One cached draw: everything a pass needs to issue it without looking at the proxy.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DrawPacket {
    /// Pipeline variant within the pass: `masked * 2 + double_sided` (prepass, shadow) or
    /// `double_sided` (opaque, transparent).
    pub variant: u8,
    pub material: Option<AssetId<MaterialAsset>>,
    pub mesh: MeshKind,
    pub slot: u32,
}

#[derive(Debug, Default)]
pub(crate) struct DrawBins {
    pub prepass: Vec<DrawPacket>,
    pub opaque: Vec<DrawPacket>,
    pub shadow: Vec<DrawPacket>,
    /// Unsorted; the frame sorts it back to front.
    pub transparent: Vec<DrawPacket>,
}

/// Counters for tests and stats.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub proxies: usize,
    /// Object-buffer bytes uploaded by the last frame.
    pub object_bytes_written: u64,
    /// Object-buffer writes (contiguous runs) by the last frame.
    pub object_writes: u32,
    /// Bin rebuilds since the renderer was created.
    pub bin_rebuilds: u64,
}

/// What a proxy's bins depend on. A change outside these fields (transform, tint, roughness…)
/// only rewrites the proxy's object slot.
#[derive(PartialEq)]
struct BinKey {
    mesh: MeshKind,
    material: Option<AssetId<MaterialAsset>>,
    alpha: u8,
    double_sided: bool,
    cast_shadow: bool,
    shadow_double_sided: bool,
    transparent_shadows: bool,
}

impl BinKey {
    fn of(item: &DrawItem) -> Self {
        Self {
            mesh: item.mesh,
            material: item.material_asset,
            alpha: match item.alpha {
                DrawAlpha::Opaque => 0,
                DrawAlpha::Mask(_) => 1,
                DrawAlpha::Blend => 2,
            },
            double_sided: item.double_sided,
            cast_shadow: item.cast_shadow,
            shadow_double_sided: item.shadow_double_sided,
            transparent_shadows: item.transparent_shadows,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RenderScene {
    slots: Vec<Option<DrawItem>>,
    keys: HashMap<ProxyKey, u32>,
    free: Vec<u32>,
    dirty: Vec<u32>,
    bins_dirty: bool,
    pub bins: DrawBins,
    stats: SceneStats,
    warned_full: bool,
}

impl RenderScene {
    pub(crate) fn item(&self, slot: u32) -> Option<&DrawItem> {
        self.slots.get(slot as usize).and_then(Option::as_ref)
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    pub(crate) fn stats(&self) -> SceneStats {
        SceneStats {
            proxies: self.len(),
            ..self.stats
        }
    }

    /// Live proxies, in slot order.
    pub(crate) fn items(&self) -> impl Iterator<Item = &DrawItem> {
        self.slots.iter().flatten()
    }

    pub(crate) fn apply(&mut self, key: ProxyKey, item: Option<DrawItem>) {
        match (item, self.keys.get(&key).copied()) {
            (Some(item), Some(slot)) => {
                let old = self.slots[slot as usize].replace(item);
                if old.as_ref() == Some(&item) {
                    return;
                }
                if old.is_none_or(|o| BinKey::of(&o) != BinKey::of(&item)) {
                    self.bins_dirty = true;
                }
                self.dirty.push(slot);
            }
            (Some(item), None) => {
                let slot = match self.free.pop() {
                    Some(s) => s,
                    None if self.slots.len() < MAX_OBJECTS => {
                        self.slots.push(None);
                        (self.slots.len() - 1) as u32
                    }
                    None => {
                        if !self.warned_full {
                            warn!(max = MAX_OBJECTS, "too many render proxies; extra skipped");
                            self.warned_full = true;
                        }
                        return;
                    }
                };
                self.slots[slot as usize] = Some(item);
                self.keys.insert(key, slot);
                self.dirty.push(slot);
                self.bins_dirty = true;
            }
            (None, Some(slot)) => {
                self.keys.remove(&key);
                self.slots[slot as usize] = None;
                self.free.push(slot);
                self.bins_dirty = true;
            }
            (None, None) => {}
        }
    }

    /// The immediate path (`FrameInputs::items`, used by tests and tools): the slice *is*
    /// the scene, keyed by index. Unchanged items cost nothing, as with deltas.
    pub(crate) fn sync_items(&mut self, items: &[DrawItem]) {
        let stale: Vec<ProxyKey> = self
            .keys
            .keys()
            .filter(|k| k.0 >= items.len() as u64)
            .copied()
            .collect();
        for key in stale {
            self.apply(key, None);
        }
        for (i, item) in items.iter().enumerate() {
            self.apply(ProxyKey(i as u64), Some(*item));
        }
    }

    /// Takes the slots to upload this frame, sorted, as contiguous runs `(first, count)`.
    pub(crate) fn take_dirty_runs(&mut self) -> Vec<(u32, u32)> {
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.sort_unstable();
        dirty.dedup();
        dirty.retain(|&s| self.slots[s as usize].is_some());
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for slot in dirty {
            match runs.last_mut() {
                Some((first, count)) if *first + *count == slot => *count += 1,
                _ => runs.push((slot, 1)),
            }
        }
        runs
    }

    pub(crate) fn record_upload(&mut self, writes: u32, bytes: u64) {
        self.stats.object_writes = writes;
        self.stats.object_bytes_written = bytes;
    }

    /// Rebuilds the bins if membership or a bin key changed since the last call.
    pub(crate) fn update_bins(&mut self) {
        if !self.bins_dirty {
            return;
        }
        self.bins_dirty = false;
        self.stats.bin_rebuilds += 1;
        let bins = &mut self.bins;
        bins.prepass.clear();
        bins.opaque.clear();
        bins.shadow.clear();
        bins.transparent.clear();
        for (slot, item) in self.slots.iter().enumerate() {
            let Some(item) = item else { continue };
            let packet = |variant: u8| DrawPacket {
                variant,
                material: item.material_asset,
                mesh: item.mesh,
                slot: slot as u32,
            };
            let double = u8::from(item.double_sided);
            let blended = item.alpha == DrawAlpha::Blend;
            let masked = matches!(item.alpha, DrawAlpha::Mask(_));
            if blended {
                bins.transparent.push(packet(double));
            } else {
                bins.prepass.push(packet(u8::from(masked) * 2 + double));
                bins.opaque.push(packet(double));
            }
            if item.cast_shadow && (!blended || item.transparent_shadows) {
                let shadow_double = u8::from(item.double_sided || item.shadow_double_sided);
                bins.shadow
                    .push(packet(u8::from(masked || blended) * 2 + shadow_double));
            }
        }
        // Pipeline first, then material: the two state changes a pass pays for.
        let order = |p: &DrawPacket| (p.variant, p.material.map(|m| (m.index(), m.generation())));
        bins.prepass.sort_by_key(order);
        bins.opaque.sort_by_key(order);
        bins.shadow.sort_by_key(order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3, Vec4};

    fn cube(x: f32) -> DrawItem {
        DrawItem::new(
            MeshKind::Cube,
            Mat4::from_translation(Vec3::X * x),
            Vec4::ONE,
        )
    }

    #[test]
    fn delta_keeps_only_the_latest_change_per_key() {
        let mut d = SceneDelta::default();
        d.upsert(ProxyKey(1), cube(0.0));
        d.upsert(ProxyKey(1), cube(2.0));
        d.upsert(ProxyKey(2), cube(0.0));
        d.remove(ProxyKey(2));
        assert_eq!(d.len(), 2);
        assert_eq!(d.get(ProxyKey(1)), Some(Some(&cube(2.0))));
        assert_eq!(d.removes().collect::<Vec<_>>(), [ProxyKey(2)]);
    }

    #[test]
    fn static_scene_uploads_and_rebuilds_nothing() {
        let mut s = RenderScene::default();
        let items = [cube(0.0), cube(1.0), cube(2.0)];
        s.sync_items(&items);
        assert_eq!(s.take_dirty_runs(), [(0, 3)], "first frame: one write");
        s.update_bins();
        assert_eq!(s.stats().bin_rebuilds, 1);

        s.sync_items(&items);
        assert!(s.take_dirty_runs().is_empty());
        s.update_bins();
        assert_eq!(
            s.stats().bin_rebuilds,
            1,
            "no rebuild for an unchanged scene"
        );
    }

    #[test]
    fn moving_rewrites_one_slot_without_rebuilding_bins() {
        let mut s = RenderScene::default();
        s.sync_items(&[cube(0.0), cube(1.0), cube(2.0)]);
        s.take_dirty_runs();
        s.update_bins();
        s.sync_items(&[cube(0.0), cube(5.0), cube(2.0)]);
        assert_eq!(s.take_dirty_runs(), [(1, 1)]);
        s.update_bins();
        assert_eq!(s.stats().bin_rebuilds, 1);

        // Turning it blended moves it to another bin.
        let mut glass = cube(5.0);
        glass.alpha = DrawAlpha::Blend;
        s.sync_items(&[cube(0.0), glass, cube(2.0)]);
        s.update_bins();
        assert_eq!(s.stats().bin_rebuilds, 2);
        assert_eq!(s.bins.transparent.len(), 1);
        assert_eq!(s.bins.opaque.len(), 2);
    }

    #[test]
    fn removed_slots_are_reused_and_bins_follow() {
        let mut s = RenderScene::default();
        s.apply(ProxyKey(10), Some(cube(0.0)));
        s.apply(ProxyKey(11), Some(cube(1.0)));
        s.apply(ProxyKey(10), None);
        s.apply(ProxyKey(12), Some(cube(2.0)));
        assert_eq!(s.len(), 2);
        assert_eq!(s.keys[&ProxyKey(12)], 0, "freed slot reused");
        s.update_bins();
        assert_eq!(s.bins.opaque.len(), 2);
        // Removing an unknown key is a no-op.
        s.apply(ProxyKey(99), None);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn bins_sort_by_pipeline_then_material_and_split_shadows() {
        let mat = |i| Some(AssetId::<MaterialAsset>::from_raw(i, 0));
        let mut a = cube(0.0);
        a.material_asset = mat(2);
        let mut b = cube(1.0);
        b.material_asset = mat(1);
        let mut c = cube(2.0);
        c.double_sided = true;
        c.cast_shadow = false;
        let mut s = RenderScene::default();
        s.sync_items(&[a, b, c]);
        s.update_bins();
        let opaque: Vec<_> = s.bins.opaque.iter().map(|p| (p.variant, p.slot)).collect();
        assert_eq!(
            opaque,
            [(0, 1), (0, 0), (1, 2)],
            "single-sided by material, then double"
        );
        assert_eq!(s.bins.shadow.len(), 2, "c doesn't cast shadows");
    }

    #[test]
    fn scene_is_capped_at_max_objects() {
        let mut s = RenderScene::default();
        for i in 0..MAX_OBJECTS as u64 + 5 {
            s.apply(ProxyKey(i), Some(cube(0.0)));
        }
        assert_eq!(s.len(), MAX_OBJECTS);
    }
}
