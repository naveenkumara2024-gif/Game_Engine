//! Gameplay-facing render components and the **extract** step (law 2, lore §6.7).
//!
//! Gameplay only ever touches components. Once per frame, in the `Extract` tick group,
//! [`extract_render_snapshot`] copies what the renderer needs into a plain-data
//! [`RenderSnapshot`]. That is the single sync point between game and render state; in M5 the
//! snapshot moves to the render thread unchanged.

use ge_ecs::prelude::*;

use crate::{Camera, DrawItem, MeshKind};

/// Draws a built-in mesh at the entity's `GlobalTransform`.
#[derive(Component, Copy, Clone, Debug)]
#[require(Transform)]
pub struct MeshRenderer {
    pub mesh: MeshKind,
    /// Linear RGB multiplier.
    pub tint: Vec4,
}

impl MeshRenderer {
    pub fn new(mesh: MeshKind, tint: [f32; 3]) -> Self {
        Self {
            mesh,
            tint: Vec3::from(tint).extend(1.0),
        }
    }
}

/// A perspective camera looking down the entity's local -Z. The first one found is used.
#[derive(Component, Copy, Clone, Debug)]
#[require(Transform)]
pub struct Camera3d {
    pub fov_y: f32,
    pub near: f32,
}

impl Default for Camera3d {
    fn default() -> Self {
        let d = Camera::default();
        Self {
            fov_y: d.fov_y,
            near: d.near,
        }
    }
}

/// Plain-data copy of everything the renderer needs for one frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct RenderSnapshot {
    pub camera: Option<Camera>,
    pub time: f32,
    pub items: Vec<DrawItem>,
}

pub fn extract_render_snapshot(
    mut snapshot: ResMut<RenderSnapshot>,
    time: Res<Time>,
    cameras: Query<(&GlobalTransform, &Camera3d)>,
    meshes: Query<(&GlobalTransform, &MeshRenderer)>,
) {
    let snapshot = &mut *snapshot;
    snapshot.time = time.elapsed_secs();
    snapshot.camera = cameras.iter().next().map(|(global, cam)| {
        let position = global.translation();
        Camera {
            position,
            target: position + global.forward(),
            up: global.up(),
            fov_y: cam.fov_y,
            near: cam.near,
        }
    });
    snapshot.items.clear();
    snapshot
        .items
        .extend(meshes.iter().map(|(global, mesh)| DrawItem {
            mesh: mesh.mesh,
            world_from_local: global.to_mat4(),
            tint: mesh.tint,
        }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ge_ecs::propagate_transforms;
    use ge_ecs::schedule::Schedule;

    #[test]
    fn extract_copies_cameras_and_meshes() {
        let mut world = World::new();
        world.init_resource::<RenderSnapshot>();
        world.init_resource::<Time>();
        world.spawn((
            Transform::from_xyz(0.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            Camera3d::default(),
        ));
        let parent = world
            .spawn((
                Transform::from_xyz(1.0, 0.0, 0.0),
                MeshRenderer::new(MeshKind::Cube, [1.0; 3]),
            ))
            .id();
        world.spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            MeshRenderer::new(MeshKind::Plane, [0.5; 3]),
            ChildOf(parent),
        ));

        let mut schedule = Schedule::default();
        schedule.add_systems((propagate_transforms, extract_render_snapshot).chain());
        schedule.run(&mut world);

        let snap = world.resource::<RenderSnapshot>();
        let cam = snap.camera.expect("camera extracted");
        assert!(
            (cam.target - cam.position)
                .normalize()
                .abs_diff_eq((Vec3::ZERO - Vec3::new(0.0, 2.0, 5.0)).normalize(), 1e-5)
        );
        assert_eq!(snap.items.len(), 2);
        let child = snap
            .items
            .iter()
            .find(|i| i.mesh == MeshKind::Plane)
            .unwrap();
        assert_eq!(
            child.world_from_local.w_axis.truncate(),
            Vec3::new(1.0, 1.0, 0.0),
            "child inherits the parent's transform"
        );
    }
}
