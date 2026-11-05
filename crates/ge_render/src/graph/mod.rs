//! Render graph (plan 0008 step 1; lore §3.7 RDG, §6.7). Each frame the renderer declares its
//! passes and the resources they read and write; the graph then
//!
//! 1. **validates** the order (a transient may not be read before something writes it),
//! 2. **culls** passes whose results nobody consumes (walking back from the outputs),
//! 3. computes each transient texture's **lifetime** (first..last surviving pass that uses it),
//! 4. **aliases** transients: a later texture with an identical [`TransientDesc`] whose lifetime
//!    starts after an earlier one's ends reuses the same physical texture, and
//! 5. **executes** the surviving passes in declaration order, opening each RHI pass with its
//!    `&'static str` name (debug label + GPU timestamp).
//!
//! Aliasing here is *whole-texture reuse*: wgpu has no placed resources, so textures with
//! different descriptors can't overlap in memory. A native backend can alias memory behind the
//! same API (plan 0008, decision 2). Barriers are wgpu's job; the declared reads and writes are
//! what a native backend would derive them from.
//!
//! Pass order is the order passes are added: no reordering beyond culling (plan 0008,
//! decision 3), so a frame stays easy to read in RenderDoc.
//!
//! **Rule for pass authors:** declare every graph resource a pass touches. A transient that
//! is used without being declared may be aliased with another texture while still in use.

mod pool;

use std::collections::HashSet;

use ge_rhi::{
    ClearColor, ColorTarget, ComputePass, DepthTarget, Frame, LoadOp, RenderPass, Rhi,
    TargetPassDesc, TextureDesc, TextureFormat, TextureUsage, TextureView,
};
use tracing::info_span;

pub(crate) use pool::TransientPool;

/// What a transient texture is; textures with equal descriptors may share memory.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct TransientDesc {
    pub size: (u32, u32),
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

impl TransientDesc {
    /// A render target that later passes can also sample.
    pub fn target(size: (u32, u32), format: TextureFormat) -> Self {
        Self {
            size,
            format,
            usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::TEXTURE_BINDING,
        }
    }

    fn bytes(&self) -> u64 {
        u64::from(self.size.0) * u64::from(self.size.1) * u64::from(self.format.bytes_per_pixel())
    }
}

/// A texture in the graph (transient or imported). Copy it into pass closures freely.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct GraphTexture(u32);

impl GraphTexture {
    /// One of an imported texture's views (e.g. a cascade layer). Writing a sub-view is a
    /// partial write: it never makes earlier writers of the texture dead.
    pub fn subview(self, index: u32) -> TexRef {
        TexRef {
            texture: self,
            sub: index,
        }
    }
}

/// A buffer the graph tracks for ordering and culling only; passes bind the real buffer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct GraphBuffer(u32);

/// A texture plus which of its views a pass attaches.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct TexRef {
    pub texture: GraphTexture,
    pub sub: u32,
}

impl From<GraphTexture> for TexRef {
    fn from(texture: GraphTexture) -> Self {
        texture.subview(0)
    }
}

/// Anything a pass can declare a read or write on.
pub trait GraphResource: Copy {
    #[doc(hidden)]
    fn index(self) -> u32;
}
impl GraphResource for GraphTexture {
    fn index(self) -> u32 {
        self.0
    }
}
impl GraphResource for GraphBuffer {
    fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("pass '{pass}' reads '{resource}' before any pass writes it")]
    ReadBeforeWrite {
        pass: &'static str,
        resource: &'static str,
    },
    #[error("pass '{pass}' uses a view {sub} of '{resource}' that doesn't exist")]
    BadSubresource {
        pass: &'static str,
        resource: &'static str,
        sub: u32,
    },
}

/// What a compiled frame did, for stats (`r.Graph.Stats`) and tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphReport {
    /// Surviving passes, in execution order.
    pub executed: Vec<&'static str>,
    pub culled: Vec<&'static str>,
    /// Transient textures the surviving passes use, and the physical textures backing them.
    pub virtual_textures: usize,
    pub physical_textures: usize,
    pub virtual_bytes: u64,
    pub physical_bytes: u64,
}

enum ResourceKind {
    Transient(TransientDesc),
    /// Persistent textures owned elsewhere (shadow maps, the backbuffer), one view per `sub`.
    Imported(Vec<TextureView>),
    Buffer,
}

struct Resource {
    name: &'static str,
    kind: ResourceKind,
    output: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Access {
    Read,
    /// Overwrites the whole resource: earlier contents are dead.
    Overwrite,
    /// Reads and writes (LoadOp::Load attachments, partial writes).
    Modify,
}

impl Access {
    fn reads(self) -> bool {
        matches!(self, Self::Read | Self::Modify)
    }
    fn writes(self) -> bool {
        matches!(self, Self::Overwrite | Self::Modify)
    }
}

type RasterFn<'a> = Box<dyn FnOnce(&mut RenderPass<'_>, &PassResources<'_>) + 'a>;
type ComputeFn<'a> = Box<dyn FnOnce(&mut ComputePass<'_>, &PassResources<'_>) + 'a>;

enum Exec<'a> {
    Raster {
        colors: Vec<(TexRef, LoadOp<ClearColor>)>,
        depth: Option<(TexRef, LoadOp<f32>, bool)>,
        run: RasterFn<'a>,
    },
    Compute(ComputeFn<'a>),
}

struct PassNode<'a> {
    name: &'static str,
    uses: Vec<(u32, Access, Option<u32>)>,
    side_effect: bool,
    exec: Exec<'a>,
}

/// One frame's passes and resources. Build, then [`RenderGraph::execute`].
#[derive(Default)]
pub struct RenderGraph<'a> {
    resources: Vec<Resource>,
    passes: Vec<PassNode<'a>>,
}

impl std::fmt::Debug for RenderGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderGraph")
            .field("resources", &self.resources.len())
            .field(
                "passes",
                &self.passes.iter().map(|p| p.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Resolved views for the running pass.
#[derive(Debug)]
pub struct PassResources<'r> {
    views: &'r [Vec<TextureView>],
}

impl PassResources<'_> {
    /// The texture's (first) view. Panics only if the pass didn't declare the texture, which
    /// is a bug in the pass (see the module docs).
    pub fn texture(&self, texture: GraphTexture) -> &TextureView {
        self.view(texture.into())
    }

    pub fn view(&self, r: TexRef) -> &TextureView {
        self.views
            .get(r.texture.0 as usize)
            .and_then(|v| v.get(r.sub as usize))
            .expect("render graph: pass used a texture it didn't declare")
    }
}

impl<'a> RenderGraph<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    fn push_resource(&mut self, name: &'static str, kind: ResourceKind, output: bool) -> u32 {
        self.resources.push(Resource { name, kind, output });
        (self.resources.len() - 1) as u32
    }

    /// A texture that lives only within this frame, allocated (and aliased) by the graph.
    pub fn create_texture(&mut self, name: &'static str, desc: TransientDesc) -> GraphTexture {
        GraphTexture(self.push_resource(name, ResourceKind::Transient(desc), false))
    }

    /// A persistent texture owned outside the graph; `views[k]` is what `.subview(k)` attaches.
    pub fn import_texture(&mut self, name: &'static str, views: &[TextureView]) -> GraphTexture {
        GraphTexture(self.push_resource(name, ResourceKind::Imported(views.to_vec()), false))
    }

    /// The frame's display target: an output, so the passes that produce it survive culling.
    pub fn import_backbuffer(&mut self, view: TextureView) -> GraphTexture {
        GraphTexture(self.push_resource("Backbuffer", ResourceKind::Imported(vec![view]), true))
    }

    /// A buffer tracked for ordering and culling (the pass binds the real one).
    pub fn import_buffer(&mut self, name: &'static str) -> GraphBuffer {
        GraphBuffer(self.push_resource(name, ResourceKind::Buffer, false))
    }

    /// Keeps whatever writes `resource` alive (e.g. an offscreen target read back later).
    pub fn mark_output(&mut self, resource: impl GraphResource) {
        if let Some(r) = self.resources.get_mut(resource.index() as usize) {
            r.output = true;
        }
    }

    pub fn raster_pass(&mut self, name: &'static str) -> RasterPassBuilder<'_, 'a> {
        RasterPassBuilder {
            graph: self,
            name,
            uses: Vec::new(),
            side_effect: false,
            colors: Vec::new(),
            depth: None,
        }
    }

    pub fn compute_pass(&mut self, name: &'static str) -> ComputePassBuilder<'_, 'a> {
        ComputePassBuilder {
            graph: self,
            name,
            uses: Vec::new(),
            side_effect: false,
        }
    }

    fn is_partial(&self, r: TexRef) -> bool {
        matches!(&self.resources[r.texture.0 as usize].kind, ResourceKind::Imported(v) if v.len() > 1)
    }

    /// Validates, culls and allocates — everything except touching the GPU.
    pub fn compile(&self) -> Result<CompiledGraph, GraphError> {
        // 1. Validation: reads of transients need an earlier writer; sub-views must exist.
        let mut written = HashSet::new();
        for pass in &self.passes {
            for &(r, access, sub) in &pass.uses {
                let res = &self.resources[r as usize];
                if let (Some(sub), ResourceKind::Imported(views)) = (sub, &res.kind)
                    && sub as usize >= views.len()
                {
                    return Err(GraphError::BadSubresource {
                        pass: pass.name,
                        resource: res.name,
                        sub,
                    });
                }
                if access.reads()
                    && matches!(res.kind, ResourceKind::Transient(_))
                    && !written.contains(&r)
                {
                    return Err(GraphError::ReadBeforeWrite {
                        pass: pass.name,
                        resource: res.name,
                    });
                }
            }
            written.extend(pass.uses.iter().filter(|u| u.1.writes()).map(|u| u.0));
        }

        // 2. Culling, walking back from the outputs.
        let mut live: HashSet<u32> = (0..self.resources.len() as u32)
            .filter(|&r| self.resources[r as usize].output)
            .collect();
        let mut needed = vec![false; self.passes.len()];
        for (i, pass) in self.passes.iter().enumerate().rev() {
            let keep = pass.side_effect
                || pass
                    .uses
                    .iter()
                    .any(|&(r, a, _)| a.writes() && live.contains(&r));
            if !keep {
                continue;
            }
            needed[i] = true;
            for &(r, access, _) in &pass.uses {
                if access == Access::Overwrite {
                    live.remove(&r);
                }
            }
            for &(r, access, _) in &pass.uses {
                if access.reads() {
                    live.insert(r);
                }
            }
        }

        // 3. Lifetimes of transients over the surviving passes.
        let mut span: Vec<Option<(usize, usize)>> = vec![None; self.resources.len()];
        for (i, pass) in self.passes.iter().enumerate().filter(|(i, _)| needed[*i]) {
            for &(r, ..) in &pass.uses {
                if matches!(self.resources[r as usize].kind, ResourceKind::Transient(_)) {
                    let s = span[r as usize].get_or_insert((i, i));
                    s.1 = i;
                }
            }
        }

        // 4. Aliasing: greedy, in order of first use. `slots` = (desc, busy until pass).
        let mut order: Vec<u32> = (0..self.resources.len() as u32)
            .filter(|&r| span[r as usize].is_some())
            .collect();
        order.sort_by_key(|&r| span[r as usize].map(|s| s.0));
        let mut slots: Vec<(TransientDesc, usize)> = Vec::new();
        let mut physical = vec![None; self.resources.len()];
        let mut report = GraphReport::default();
        for r in order {
            let ResourceKind::Transient(desc) = self.resources[r as usize].kind else {
                continue;
            };
            let Some((first, last)) = span[r as usize] else {
                continue;
            };
            report.virtual_textures += 1;
            report.virtual_bytes += desc.bytes();
            let free = slots
                .iter()
                .position(|&(d, busy_until)| d == desc && busy_until < first);
            let slot = match free {
                Some(s) => {
                    slots[s].1 = last;
                    s
                }
                None => {
                    report.physical_textures += 1;
                    report.physical_bytes += desc.bytes();
                    slots.push((desc, last));
                    slots.len() - 1
                }
            };
            // The ordinal among same-desc slots keys the cross-frame pool.
            let ordinal = slots[..slot].iter().filter(|(d, _)| *d == desc).count() as u32;
            physical[r as usize] = Some((desc, ordinal));
        }

        for (pass, &keep) in self.passes.iter().zip(&needed) {
            if keep {
                report.executed.push(pass.name);
            } else {
                report.culled.push(pass.name);
            }
        }
        Ok(CompiledGraph {
            needed,
            physical,
            report,
        })
    }

    /// Compiles, allocates transients from `pool`, and records the surviving passes.
    pub(crate) fn execute(
        self,
        rhi: &Rhi,
        frame: &mut Frame,
        pool: &mut TransientPool<ge_rhi::Texture>,
    ) -> Result<GraphReport, GraphError> {
        let _span = info_span!("Render::Graph", passes = self.passes.len()).entered();
        let compiled = self.compile()?;
        pool.begin_frame();
        let views: Vec<Vec<TextureView>> = self
            .resources
            .iter()
            .zip(&compiled.physical)
            .map(|(res, phys)| match (&res.kind, phys) {
                (ResourceKind::Imported(v), _) => v.clone(),
                (ResourceKind::Transient(_), Some((desc, ordinal))) => {
                    let texture = pool.acquire((*desc, *ordinal), || {
                        rhi.create_texture(&TextureDesc {
                            label: res.name,
                            size: desc.size,
                            format: desc.format,
                            usage: desc.usage,
                        })
                    });
                    vec![texture.full_view()]
                }
                _ => Vec::new(),
            })
            .collect();
        pool.evict_idle();
        let resources = PassResources { views: &views };

        for (pass, keep) in self.passes.into_iter().zip(compiled.needed) {
            if !keep {
                continue;
            }
            match pass.exec {
                Exec::Raster { colors, depth, run } => {
                    let color: Vec<ColorTarget<'_>> = colors
                        .iter()
                        .map(|&(r, load)| ColorTarget {
                            view: resources.view(r),
                            load,
                        })
                        .collect();
                    let depth = depth.map(|(r, load, store)| DepthTarget {
                        view: resources.view(r),
                        load,
                        store,
                    });
                    let mut rp = frame.pass_to(&TargetPassDesc {
                        label: pass.name,
                        color: &color,
                        depth,
                    });
                    run(&mut rp, &resources);
                }
                Exec::Compute(run) => {
                    let mut cp = frame.compute_pass(pass.name);
                    run(&mut cp, &resources);
                }
            }
        }
        Ok(compiled.report)
    }
}

/// The result of [`RenderGraph::compile`].
#[derive(Debug)]
pub struct CompiledGraph {
    needed: Vec<bool>,
    /// Per resource: the pool key of the physical texture backing a surviving transient.
    physical: Vec<Option<(TransientDesc, u32)>>,
    pub report: GraphReport,
}

impl CompiledGraph {
    /// Whether two textures share one physical texture this frame.
    pub fn aliased(&self, a: GraphTexture, b: GraphTexture) -> bool {
        let (pa, pb) = (&self.physical[a.0 as usize], &self.physical[b.0 as usize]);
        pa.is_some() && pa == pb
    }
}

fn load_access<T>(load: &LoadOp<T>) -> Access {
    match load {
        LoadOp::Clear(_) => Access::Overwrite,
        LoadOp::Load => Access::Modify,
    }
}

/// Declares a raster pass; [`RasterPassBuilder::run`] adds it.
pub struct RasterPassBuilder<'g, 'a> {
    graph: &'g mut RenderGraph<'a>,
    name: &'static str,
    uses: Vec<(u32, Access, Option<u32>)>,
    side_effect: bool,
    colors: Vec<(TexRef, LoadOp<ClearColor>)>,
    depth: Option<(TexRef, LoadOp<f32>, bool)>,
}

impl std::fmt::Debug for RasterPassBuilder<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasterPassBuilder")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<'a> RasterPassBuilder<'_, 'a> {
    fn attach(&mut self, r: TexRef, mut access: Access) {
        if access == Access::Overwrite && self.graph.is_partial(r) {
            access = Access::Modify;
        }
        self.uses.push((r.texture.0, access, Some(r.sub)));
    }

    pub fn color(mut self, target: impl Into<TexRef>, load: LoadOp<ClearColor>) -> Self {
        let r = target.into();
        self.attach(r, load_access(&load));
        self.colors.push((r, load));
        self
    }

    /// `store = false` discards depth after the pass (the attachment is still a write).
    pub fn depth(mut self, target: impl Into<TexRef>, load: LoadOp<f32>, store: bool) -> Self {
        let r = target.into();
        self.attach(r, load_access(&load));
        self.depth = Some((r, load, store));
        self
    }

    /// Samples or binds `resource` (shadow maps, cluster lists, SceneColor…).
    pub fn read(mut self, resource: impl GraphResource) -> Self {
        self.uses.push((resource.index(), Access::Read, None));
        self
    }

    /// Never culled (readbacks, debug captures).
    pub fn side_effect(mut self) -> Self {
        self.side_effect = true;
        self
    }

    pub fn run(self, run: impl FnOnce(&mut RenderPass<'_>, &PassResources<'_>) + 'a) {
        self.graph.passes.push(PassNode {
            name: self.name,
            uses: self.uses,
            side_effect: self.side_effect,
            exec: Exec::Raster {
                colors: self.colors,
                depth: self.depth,
                run: Box::new(run),
            },
        });
    }
}

/// Declares a compute pass; [`ComputePassBuilder::run`] adds it.
pub struct ComputePassBuilder<'g, 'a> {
    graph: &'g mut RenderGraph<'a>,
    name: &'static str,
    uses: Vec<(u32, Access, Option<u32>)>,
    side_effect: bool,
}

impl std::fmt::Debug for ComputePassBuilder<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputePassBuilder")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<'a> ComputePassBuilder<'_, 'a> {
    pub fn read(mut self, resource: impl GraphResource) -> Self {
        self.uses.push((resource.index(), Access::Read, None));
        self
    }

    /// Overwrites `resource` entirely.
    pub fn write(mut self, resource: impl GraphResource) -> Self {
        self.uses.push((resource.index(), Access::Overwrite, None));
        self
    }

    /// Reads and writes `resource` (partial update).
    pub fn modify(mut self, resource: impl GraphResource) -> Self {
        self.uses.push((resource.index(), Access::Modify, None));
        self
    }

    pub fn side_effect(mut self) -> Self {
        self.side_effect = true;
        self
    }

    pub fn run(self, run: impl FnOnce(&mut ComputePass<'_>, &PassResources<'_>) + 'a) {
        self.graph.passes.push(PassNode {
            name: self.name,
            uses: self.uses,
            side_effect: self.side_effect,
            exec: Exec::Compute(Box::new(run)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDR: TextureFormat = TextureFormat::Rgba16Float;

    fn desc(w: u32) -> TransientDesc {
        TransientDesc::target((w, w), HDR)
    }

    /// Imported views need a GPU; tests use a buffer as the "backbuffer" output instead.
    fn output(g: &mut RenderGraph<'_>) -> GraphBuffer {
        let out = g.import_buffer("Out");
        g.mark_output(out);
        out
    }

    #[test]
    fn reading_a_transient_before_writing_it_is_an_error() {
        let mut g = RenderGraph::new();
        let t = g.create_texture("T", desc(4));
        g.compute_pass("Early").read(t).side_effect().run(|_, _| {});
        g.compute_pass("Late").write(t).run(|_, _| {});
        assert_eq!(
            g.compile().unwrap_err(),
            GraphError::ReadBeforeWrite {
                pass: "Early",
                resource: "T"
            }
        );
    }

    #[test]
    fn a_load_attachment_counts_as_a_read() {
        let mut g = RenderGraph::new();
        let t = g.create_texture("T", desc(4));
        g.raster_pass("Blend").color(t, LoadOp::Load).run(|_, _| {});
        assert!(matches!(
            g.compile(),
            Err(GraphError::ReadBeforeWrite { pass: "Blend", .. })
        ));
    }

    #[test]
    fn passes_nobody_consumes_are_culled() {
        let mut g = RenderGraph::new();
        let out = output(&mut g);
        let used = g.create_texture("Used", desc(4));
        let unused = g.create_texture("Unused", desc(4));
        g.raster_pass("MakesUsed")
            .color(used, LoadOp::Clear(ClearColor([0.0; 4])))
            .run(|_, _| {});
        g.raster_pass("MakesUnused")
            .color(unused, LoadOp::Clear(ClearColor([0.0; 4])))
            .run(|_, _| {});
        g.compute_pass("Final").read(used).write(out).run(|_, _| {});
        let c = g.compile().unwrap();
        assert_eq!(c.report.executed, ["MakesUsed", "Final"]);
        assert_eq!(c.report.culled, ["MakesUnused"]);
        // The culled pass's texture isn't allocated.
        assert_eq!(c.report.virtual_textures, 1);
    }

    #[test]
    fn culling_cascades_and_overwrites_kill_earlier_writers() {
        let mut g = RenderGraph::new();
        let out = output(&mut g);
        let a = g.create_texture("A", desc(4));
        let b = g.create_texture("B", desc(4));
        g.compute_pass("WriteA").write(a).run(|_, _| {});
        // Only feeds the culled pass below, so it goes too.
        g.compute_pass("AToB").read(a).write(b).run(|_, _| {});
        g.compute_pass("Dead").read(b).run(|_, _| {});
        g.compute_pass("OverwriteOut1").write(out).run(|_, _| {});
        g.compute_pass("OverwriteOut2").write(out).run(|_, _| {});
        let c = g.compile().unwrap();
        assert_eq!(c.report.executed, ["OverwriteOut2"]);
    }

    #[test]
    fn modify_keeps_the_earlier_writer() {
        let mut g = RenderGraph::new();
        let out = output(&mut g);
        g.compute_pass("Base").write(out).run(|_, _| {});
        g.compute_pass("Overlay").modify(out).run(|_, _| {});
        assert_eq!(g.compile().unwrap().report.executed, ["Base", "Overlay"]);
    }

    #[test]
    fn side_effect_passes_survive() {
        let mut g = RenderGraph::new();
        g.compute_pass("Readback").side_effect().run(|_, _| {});
        g.compute_pass("Pointless").run(|_, _| {});
        assert_eq!(g.compile().unwrap().report.executed, ["Readback"]);
    }

    #[test]
    fn disjoint_same_desc_transients_share_a_texture() {
        let mut g = RenderGraph::new();
        let out = output(&mut g);
        let a = g.create_texture("A", desc(8));
        let b = g.create_texture("B", desc(8));
        let c = g.create_texture("C", desc(8));
        let other = g.create_texture("Other", desc(16));
        g.compute_pass("P0").write(a).run(|_, _| {});
        g.compute_pass("P1")
            .read(a)
            .write(b)
            .modify(out)
            .run(|_, _| {});
        // `a` is dead after P1, so `c` (same desc) can take its texture; `b` is still live.
        g.compute_pass("P2")
            .read(b)
            .write(c)
            .write(other)
            .modify(out)
            .run(|_, _| {});
        g.compute_pass("P3")
            .read(c)
            .read(other)
            .modify(out)
            .run(|_, _| {});
        let compiled = g.compile().unwrap();
        assert!(compiled.aliased(a, c));
        assert!(!compiled.aliased(a, b), "a and b are both live in P1");
        assert!(!compiled.aliased(b, c), "b and c are both live in P2");
        assert!(!compiled.aliased(c, other), "different descriptors");
        let r = &compiled.report;
        assert_eq!((r.virtual_textures, r.physical_textures), (4, 3));
        assert_eq!(r.virtual_bytes - r.physical_bytes, desc(8).bytes());
    }

    #[test]
    fn same_pass_textures_never_alias() {
        let mut g = RenderGraph::new();
        let out = output(&mut g);
        let a = g.create_texture("A", desc(8));
        let b = g.create_texture("B", desc(8));
        g.compute_pass("Both").write(a).write(b).run(|_, _| {});
        g.compute_pass("Use")
            .read(a)
            .read(b)
            .write(out)
            .run(|_, _| {});
        let compiled = g.compile().unwrap();
        assert!(!compiled.aliased(a, b));
        assert_eq!(compiled.report.physical_textures, 2);
    }
}
