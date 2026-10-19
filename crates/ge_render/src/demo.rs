//! The M1 test scene: a 5×5 grid of spinning cubes over a ground plane. It is shared by the
//! game binary and the golden-image test so both draw exactly the same thing. It moves into the
//! sample game once there is an ECS to hold it (M2).

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::{Camera, DrawItem, MeshKind};

pub const GRID: i32 = 5;
pub const SPACING: f32 = 2.0;

pub fn demo_camera() -> Camera {
    Camera {
        position: Vec3::new(7.0, 6.0, 9.0),
        target: Vec3::new(0.0, 0.5, 0.0),
        ..Default::default()
    }
}

/// Scene at `time` seconds. Deterministic: same time, same draws.
pub fn demo_scene(time: f32) -> Vec<DrawItem> {
    let mut items = Vec::with_capacity((GRID * GRID + 1) as usize);
    items.push(DrawItem {
        mesh: MeshKind::Plane,
        world_from_local: Mat4::from_scale(Vec3::splat(GRID as f32 * SPACING * 0.75)),
        tint: Vec4::new(0.55, 0.55, 0.58, 1.0),
    });
    let half = (GRID - 1) as f32 * 0.5;
    for z in 0..GRID {
        for x in 0..GRID {
            let i = (z * GRID + x) as f32;
            let phase = i * 0.37;
            let position = Vec3::new(
                (x as f32 - half) * SPACING,
                0.75 + 0.35 * (time * 1.3 + phase).sin(),
                (z as f32 - half) * SPACING,
            );
            let rotation = Quat::from_rotation_y(time * (0.4 + 0.05 * i))
                * Quat::from_rotation_x(0.3 * (time * 0.7 + phase).sin());
            items.push(DrawItem {
                mesh: MeshKind::Cube,
                world_from_local: Mat4::from_rotation_translation(rotation, position),
                tint: palette(i / (GRID * GRID) as f32).extend(1.0),
            });
        }
    }
    items
}

/// Cheap rainbow (cosine palette), linear RGB.
fn palette(t: f32) -> Vec3 {
    let tau = std::f32::consts::TAU;
    Vec3::new(
        0.5 + 0.45 * (tau * (t + 0.00)).cos(),
        0.5 + 0.45 * (tau * (t + 0.33)).cos(),
        0.5 + 0.45 * (tau * (t + 0.67)).cos(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_is_deterministic_and_complete() {
        let a = demo_scene(1.0);
        assert_eq!(a.len(), (GRID * GRID + 1) as usize);
        let b = demo_scene(1.0);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.world_from_local, y.world_from_local);
        }
    }
}
