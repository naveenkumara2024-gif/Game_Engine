//! Perspective camera with **reversed-Z and an infinite far plane** (lore §6.7): depth is 1 at
//! the near plane and tends to 0 at infinity. Float precision is densest near 0, which reversed-Z
//! spends on distant geometry, so z-fighting stays low without a far plane at all.

use glam::camera::rh::{proj, view};
use glam::{Mat4, Vec3};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    /// Near plane distance. Only the near plane exists; far is infinite.
    pub near: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 4.0, 10.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            near: 0.1,
        }
    }
}

impl Camera {
    /// Right-handed view matrix (camera looks down -Z in view space).
    pub fn view_from_world(&self) -> Mat4 {
        view::look_at_mat4(self.position, self.target, self.up)
    }

    /// WebGPU clip space is z ∈ [0, 1] with +Y up, which is glam's `directx` convention.
    pub fn clip_from_view(&self, aspect: f32) -> Mat4 {
        proj::directx::perspective_infinite_reverse(self.fov_y, aspect, self.near)
    }

    pub fn clip_from_world(&self, aspect: f32) -> Mat4 {
        self.clip_from_view(aspect) * self.view_from_world()
    }
}

/// Width / height, safe for a zero-sized (minimized) target.
pub fn aspect_ratio((width, height): (u32, u32)) -> f32 {
    width.max(1) as f32 / height.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc_depth(camera: &Camera, world: Vec3) -> f32 {
        let clip = camera.clip_from_world(16.0 / 9.0) * world.extend(1.0);
        clip.z / clip.w
    }

    #[test]
    fn reversed_z_near_is_one_far_is_zero() {
        let cam = Camera {
            position: Vec3::ZERO,
            target: Vec3::NEG_Z,
            ..Default::default()
        };
        let near = ndc_depth(&cam, Vec3::new(0.0, 0.0, -cam.near));
        assert!((near - 1.0).abs() < 1e-5, "near plane depth {near}");

        let far = ndc_depth(&cam, Vec3::new(0.0, 0.0, -1.0e6));
        assert!((0.0..1e-6).contains(&far), "far depth {far}");

        // Monotonic: farther is smaller.
        assert!(ndc_depth(&cam, Vec3::new(0.0, 0.0, -2.0)) > ndc_depth(&cam, Vec3::NEG_Z * 3.0));
    }

    #[test]
    fn target_projects_to_screen_center() {
        let cam = Camera::default();
        let clip = cam.clip_from_world(1.5) * cam.target.extend(1.0);
        let ndc = clip / clip.w;
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5, "{ndc:?}");
        assert!(clip.w > 0.0, "target is in front of the camera");
    }

    #[test]
    fn aspect_ratio_handles_minimized() {
        assert_eq!(aspect_ratio((1920, 1080)), 1920.0 / 1080.0);
        assert_eq!(aspect_ratio((0, 0)), 1.0);
    }
}
