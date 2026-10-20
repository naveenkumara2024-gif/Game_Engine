//! The player: moves in the fixed step (deterministic), rendered interpolated between the last
//! two fixed states so motion stays smooth at any frame rate.

use ge_engine::prelude::*;

use crate::actions;
use crate::arena::{ARENA_HALF_SIZE, Spin};
use crate::camera::OrbitCamera;

const START: Vec3 = Vec3::new(0.0, 0.5, 0.0);

#[derive(Component, Copy, Clone, Debug)]
pub struct Player {
    /// Units per second.
    pub speed: f32,
    /// Yaw the body faces (radians); turns toward the movement direction.
    pub facing: f32,
}

/// Fixed-step position history used for render interpolation.
#[derive(Component, Copy, Clone, Debug, Default)]
pub struct FixedMotion {
    pub previous: Vec3,
    pub current: Vec3,
}

pub fn spawn_player(mut commands: Commands) {
    commands
        .spawn((
            Player {
                speed: 6.0,
                facing: 0.0,
            },
            FixedMotion {
                previous: START,
                current: START,
            },
            Transform::from_translation(START),
            MeshRenderer::new(MeshKind::Cube, [1.0, 0.55, 0.15]),
        ))
        .with_children(|body| {
            // Shoulders follow the body; the "antenna" on the right one also spins.
            body.spawn((
                Transform::from_xyz(-0.7, 0.3, 0.0).with_scale(Vec3::splat(0.4)),
                MeshRenderer::new(MeshKind::Cube, [0.9, 0.9, 0.9]),
            ));
            body.spawn((
                Transform::from_xyz(0.7, 0.3, 0.0).with_scale(Vec3::splat(0.4)),
                MeshRenderer::new(MeshKind::Cube, [0.9, 0.9, 0.9]),
            ))
            .with_children(|shoulder| {
                shoulder.spawn((
                    Transform::from_xyz(0.0, 1.6, 0.0).with_scale(Vec3::splat(0.6)),
                    MeshRenderer::new(MeshKind::Cube, [0.2, 0.9, 1.0]),
                    Spin {
                        axis: Vec3::Y,
                        speed: 4.0,
                    },
                ));
            });
        });
}

/// Ground-plane basis for a camera yaw: (forward, right).
pub fn camera_basis(yaw: f32) -> (Vec3, Vec3) {
    let (s, c) = yaw.sin_cos();
    (Vec3::new(-s, 0.0, -c), Vec3::new(c, 0.0, -s))
}

pub fn move_player(
    actions: Res<ActionState>,
    fixed: Res<FixedTime>,
    camera: Query<&OrbitCamera>,
    mut players: Query<(&mut Player, &mut FixedMotion)>,
) {
    let input = actions.axis2d(actions::MOVE);
    let yaw = camera.iter().next().map_or(0.0, |c| c.yaw);
    let (forward, right) = camera_basis(yaw);
    let direction = forward * input.y + right * input.x;
    let limit = ARENA_HALF_SIZE - 0.5;
    for (mut player, mut motion) in &mut players {
        motion.previous = motion.current;
        if direction != Vec3::ZERO {
            let next = motion.current + direction * player.speed * fixed.step_secs();
            motion.current = Vec3::new(
                next.x.clamp(-limit, limit),
                next.y,
                next.z.clamp(-limit, limit),
            );
            player.facing = direction.x.atan2(direction.z) + std::f32::consts::PI;
        }
    }
}

pub fn interpolate_motion(
    fixed: Res<FixedTime>,
    mut players: Query<(&Player, &FixedMotion, &mut Transform)>,
) {
    let alpha = fixed.overstep_fraction();
    for (player, motion, mut transform) in &mut players {
        transform.translation = motion.previous.lerp(motion.current, alpha);
        transform.rotation = Quat::from_rotation_y(player.facing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_basis_is_orthonormal_on_the_ground() {
        for yaw in [0.0, 0.7, -2.0, 3.1] {
            let (f, r) = camera_basis(yaw);
            assert!((f.length() - 1.0).abs() < 1e-6 && (r.length() - 1.0).abs() < 1e-6);
            assert!(f.dot(r).abs() < 1e-6);
            assert_eq!(f.y, 0.0);
            // Right-handed: right × forward = up.
            assert!(r.cross(f).abs_diff_eq(Vec3::Y, 1e-6));
        }
    }

    #[test]
    fn facing_matches_movement_direction() {
        // Player model faces -Z at facing = 0; check the formula turns -Z onto the direction.
        for dir in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Z,
            Vec3::new(1.0, 0.0, -1.0).normalize(),
        ] {
            let facing = dir.x.atan2(dir.z) + std::f32::consts::PI;
            let model_forward = Quat::from_rotation_y(facing) * Vec3::NEG_Z;
            assert!(
                model_forward.abs_diff_eq(dir, 1e-5),
                "{dir} -> {model_forward}"
            );
        }
    }
}
