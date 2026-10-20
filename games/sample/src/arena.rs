//! The arena: a floor, a ring of pillars and a cluster of spinning cubes (the M1 test scene,
//! kept as decoration).

use ge_engine::prelude::*;

/// Half-size of the walkable floor.
pub const ARENA_HALF_SIZE: f32 = 15.0;

/// Rotates the entity around a local axis at a constant rate (visual only).
#[derive(Component, Copy, Clone, Debug)]
pub struct Spin {
    pub axis: Vec3,
    /// Radians per second.
    pub speed: f32,
}

pub fn spawn_arena(mut commands: Commands) {
    commands.spawn((
        Transform::default().with_scale(Vec3::splat(ARENA_HALF_SIZE)),
        MeshRenderer::new(MeshKind::Plane, [0.55, 0.55, 0.58]),
    ));

    let pillars = 10;
    for i in 0..pillars {
        let angle = i as f32 / pillars as f32 * std::f32::consts::TAU;
        let radius = ARENA_HALF_SIZE - 2.0;
        commands.spawn((
            Transform::from_xyz(angle.cos() * radius, 2.0, angle.sin() * radius)
                .with_scale(Vec3::new(1.2, 4.0, 1.2)),
            MeshRenderer::new(MeshKind::Cube, [0.35, 0.38, 0.45]),
        ));
    }

    // Floating cluster: one parent that spins, children that spin on their own axes.
    commands
        .spawn((
            Transform::from_xyz(-7.0, 2.5, -7.0),
            Spin {
                axis: Vec3::Y,
                speed: 0.4,
            },
        ))
        .with_children(|cluster| {
            for i in 0..6 {
                let a = i as f32 / 6.0 * std::f32::consts::TAU;
                let hue = i as f32 / 6.0;
                cluster.spawn((
                    Transform::from_xyz(a.cos() * 2.0, (a * 2.0).sin() * 0.5, a.sin() * 2.0)
                        .with_scale(Vec3::splat(0.6)),
                    MeshRenderer::new(MeshKind::Cube, palette(hue)),
                    Spin {
                        axis: Vec3::new(1.0, 1.0, 0.0).normalize(),
                        speed: 1.5,
                    },
                ));
            }
        });
}

pub fn spin(time: Res<Time>, mut spinners: Query<(&Spin, &mut Transform)>) {
    let dt = time.delta_secs();
    for (spin, mut transform) in &mut spinners {
        transform.rotation =
            (transform.rotation * Quat::from_axis_angle(spin.axis, spin.speed * dt)).normalize();
    }
}

/// Cheap rainbow (cosine palette), linear RGB.
fn palette(t: f32) -> [f32; 3] {
    let tau = std::f32::consts::TAU;
    [
        0.5 + 0.45 * (tau * t).cos(),
        0.5 + 0.45 * (tau * (t + 0.33)).cos(),
        0.5 + 0.45 * (tau * (t + 0.67)).cos(),
    ]
}
