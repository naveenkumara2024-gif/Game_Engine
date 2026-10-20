//! Local [`Transform`] + world [`GlobalTransform`], propagated down the `ChildOf`/`Children`
//! hierarchy once per frame in `PostUpdate`, after gameplay and before extract (lore §6.5).

use crate::prelude::*;

/// Local transform, relative to the parent (or the world for roots).
#[derive(Component, Copy, Clone, Debug, PartialEq)]
#[require(GlobalTransform)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_xyz(x: f32, y: f32, z: f32) -> Self {
        Self::from_translation(Vec3::new(x, y, z))
    }

    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Rotates so local -Z (forward) points at `target`, keeping `up` roughly up.
    pub fn looking_at(mut self, target: Vec3, up: Vec3) -> Self {
        let forward = (target - self.translation).normalize_or_zero();
        if forward != Vec3::ZERO {
            let right = forward.cross(up).normalize_or(Vec3::X);
            let up = right.cross(forward);
            self.rotation = Quat::from_mat3(&glam::Mat3::from_cols(right, up, -forward));
        }
        self
    }

    pub fn to_affine(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// World-space transform. Written only by [`propagate_transforms`]; read it, don't set it.
#[derive(Component, Copy, Clone, Debug, PartialEq, Default)]
pub struct GlobalTransform(pub Affine3A);

impl GlobalTransform {
    pub fn translation(&self) -> Vec3 {
        self.0.translation.into()
    }

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from(self.0)
    }

    /// World-space -Z.
    pub fn forward(&self) -> Vec3 {
        self.0.transform_vector3(Vec3::NEG_Z).normalize_or_zero()
    }

    pub fn up(&self) -> Vec3 {
        self.0.transform_vector3(Vec3::Y).normalize_or_zero()
    }
}

/// Recomputes every `GlobalTransform` from the roots down.
///
/// The whole hierarchy is walked each frame, but a `GlobalTransform` is only written when its
/// value actually changed, so change detection downstream (extract) stays meaningful. That keeps
/// every case correct (reparenting, detaching, despawned parents) without dirty-bit bookkeeping;
/// it is O(entities with Transform), which is fine until scenes get large (revisit in M5).
pub fn propagate_transforms(
    roots: Query<(Entity, &Transform, Option<&Children>), Without<ChildOf>>,
    nodes: Query<(&Transform, Option<&Children>), With<ChildOf>>,
    mut globals: Query<&mut GlobalTransform>,
) {
    for (entity, transform, children) in &roots {
        let global = transform.to_affine();
        write_if_changed(&mut globals, entity, global);
        if let Some(children) = children {
            for &child in children {
                propagate_recursive(child, global, &nodes, &mut globals);
            }
        }
    }
}

fn propagate_recursive(
    entity: Entity,
    parent: Affine3A,
    nodes: &Query<(&Transform, Option<&Children>), With<ChildOf>>,
    globals: &mut Query<&mut GlobalTransform>,
) {
    let Ok((transform, children)) = nodes.get(entity) else {
        return;
    };
    let global = parent * transform.to_affine();
    write_if_changed(globals, entity, global);
    if let Some(children) = children {
        for &child in children {
            propagate_recursive(child, global, nodes, globals);
        }
    }
}

fn write_if_changed(globals: &mut Query<&mut GlobalTransform>, entity: Entity, value: Affine3A) {
    if let Ok(mut global) = globals.get_mut(entity) {
        global.set_if_neq(GlobalTransform(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::schedule::Schedule;

    fn run(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems(propagate_transforms);
        schedule.run(world);
    }

    fn global(world: &World, e: Entity) -> Vec3 {
        world.get::<GlobalTransform>(e).unwrap().translation()
    }

    #[test]
    fn three_levels_and_reparenting() {
        let mut world = World::new();
        let root = world.spawn(Transform::from_xyz(1.0, 0.0, 0.0)).id();
        let child = world
            .spawn((Transform::from_xyz(0.0, 2.0, 0.0), ChildOf(root)))
            .id();
        let grandchild = world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 3.0).with_scale(Vec3::splat(2.0)),
                ChildOf(child),
            ))
            .id();
        run(&mut world);
        assert_eq!(global(&world, root), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(global(&world, child), Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(global(&world, grandchild), Vec3::new(1.0, 2.0, 3.0));

        // Moving the root moves the whole subtree.
        world.get_mut::<Transform>(root).unwrap().translation.x = 10.0;
        run(&mut world);
        assert_eq!(global(&world, grandchild), Vec3::new(10.0, 2.0, 3.0));

        // Rotation of a parent rotates children's offsets.
        world.get_mut::<Transform>(root).unwrap().rotation =
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        run(&mut world);
        assert!(global(&world, child).abs_diff_eq(Vec3::new(8.0, 0.0, 0.0), 1e-5));

        // Reparent the grandchild to the root, then detach it entirely.
        world.entity_mut(grandchild).insert(ChildOf(root));
        world.get_mut::<Transform>(root).unwrap().rotation = Quat::IDENTITY;
        run(&mut world);
        assert_eq!(global(&world, grandchild), Vec3::new(10.0, 0.0, 3.0));
        world.entity_mut(grandchild).remove::<ChildOf>();
        run(&mut world);
        assert_eq!(global(&world, grandchild), Vec3::new(0.0, 0.0, 3.0));
    }

    #[test]
    fn unchanged_globals_are_not_marked_changed() {
        let mut world = World::new();
        let e = world.spawn(Transform::from_xyz(1.0, 2.0, 3.0)).id();
        run(&mut world);
        world.clear_trackers();
        run(&mut world);
        assert!(
            !world
                .entity(e)
                .get_ref::<GlobalTransform>()
                .unwrap()
                .is_changed()
        );
    }

    #[test]
    fn looking_at_points_forward_at_target() {
        let t = Transform::from_xyz(0.0, 5.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
        let forward = t.rotation * Vec3::NEG_Z;
        let expected = (Vec3::ZERO - t.translation).normalize();
        assert!(
            forward.abs_diff_eq(expected, 1e-5),
            "{forward} vs {expected}"
        );
    }
}
