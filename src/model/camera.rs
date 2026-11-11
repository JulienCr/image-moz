//! Orbit camera used by the wgpu preview.

use glam::camera::rh;
use glam::{Mat4, Vec2, Vec3};

use super::{Aabb, Scene, SceneCamera, UpAxis};

const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;
const FRAMING_MARGIN: f32 = 1.1;
const DEFAULT_YFOV: f32 = 0.8;
const ORBIT_RAD_PER_POINT: f32 = 0.01;
const DOLLY_STEP: f32 = 0.9;
const DOLLY_MIN: f32 = 1e-3;
const DOLLY_MAX: f32 = 1e4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub yfov: f32,
    pub up: UpAxis,
    /// Distance the dolly clamp is relative to.
    framed_distance: f32,
    /// Aspect `yfov` is defined at; narrower viewports widen the vertical FOV.
    fit_aspect: f32,
}

/// World-space (right, up, back) axes at yaw 0, pitch 0.
fn basis(up: UpAxis) -> (Vec3, Vec3, Vec3) {
    match up {
        UpAxis::Y => (Vec3::X, Vec3::Y, Vec3::Z),
        UpAxis::Z => (Vec3::X, Vec3::Z, Vec3::NEG_Y),
    }
}

fn effective_radius(bounds: &Aabb) -> f32 {
    let r = bounds.radius();
    if r.is_finite() && r > 0.0 {
        r
    } else {
        1.0
    }
}

impl OrbitCamera {
    pub fn framed(bounds: &Aabb, up: UpAxis) -> Self {
        let distance = effective_radius(bounds) / (DEFAULT_YFOV * 0.5).sin() * FRAMING_MARGIN;
        Self {
            target: bounds.center(),
            yaw: 45f32.to_radians(),
            pitch: 25f32.to_radians(),
            distance,
            yfov: DEFAULT_YFOV,
            up,
            framed_distance: distance,
            fit_aspect: 1.0,
        }
    }

    pub fn from_scene(cam: &SceneCamera, bounds: &Aabb, up: UpAxis) -> Self {
        let framed = Self::framed(bounds, up);
        let Some(forward) = (cam.target - cam.eye).try_normalize() else {
            return framed;
        };
        let (right, up_v, back) = basis(up);
        let back_dir = -forward;
        let pitch = back_dir
            .dot(up_v)
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(-MAX_PITCH, MAX_PITCH);
        let horizontal = back_dir - up_v * back_dir.dot(up_v);
        let yaw = horizontal.dot(right).atan2(horizontal.dot(back));
        let mut t = (bounds.center() - cam.eye).dot(forward);
        if !(t > 0.0) {
            t = effective_radius(bounds);
        }
        Self {
            target: cam.eye + forward * t,
            yaw,
            pitch,
            distance: t,
            yfov: if cam.yfov.is_finite() && cam.yfov > 0.01 && cam.yfov < 3.0 {
                cam.yfov
            } else {
                DEFAULT_YFOV
            },
            up,
            framed_distance: framed.distance,
            fit_aspect: cam.aspect.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(1.0),
        }
    }

    pub fn initial(scene: &Scene) -> Option<Self> {
        let bounds = scene.bounds()?;
        Some(match &scene.camera {
            Some(cam) => Self::from_scene(cam, &bounds, scene.up),
            None => Self::framed(&bounds, scene.up),
        })
    }

    pub fn eye(&self) -> Vec3 {
        let (right, up, back) = basis(self.up);
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        self.target + (up * sp + (right * sy + back * cy) * cp) * self.distance
    }

    /// Right-handed, depth 0..1; near/far hug the bounds so depth precision holds.
    pub fn view_proj(&self, aspect: f32, bounds: &Aabb) -> Mat4 {
        let eye = self.eye();
        let to_center = (bounds.center() - eye).length();
        let radius = effective_radius(bounds);
        let far = ((to_center + radius).max(self.distance + radius) * FRAMING_MARGIN).max(1e-3);
        let near = (to_center - radius).max(far * 1e-3);
        let aspect = if aspect.is_finite() && aspect > 1e-3 {
            aspect
        } else {
            1.0
        };
        let (_, up, _) = basis(self.up);
        // Keep the horizontal field of view when the viewport is narrower than the fit aspect.
        let yfov = if aspect < self.fit_aspect {
            2.0 * ((self.yfov * 0.5).tan() * self.fit_aspect / aspect).atan()
        } else {
            self.yfov
        };
        rh::proj::directx::perspective(yfov, aspect, near, far)
            * rh::view::look_at_mat4(eye, self.target, up)
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * ORBIT_RAD_PER_POINT;
        self.pitch = (self.pitch + delta.y * ORBIT_RAD_PER_POINT).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn pan(&mut self, delta: Vec2, viewport_height: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let (_, up, _) = basis(self.up);
        let right = forward.cross(up).normalize_or_zero();
        let cam_up = right.cross(forward);
        let per_point = 2.0 * self.distance * (self.yfov * 0.5).tan() / viewport_height.max(1.0);
        self.target += (-right * delta.x + cam_up * delta.y) * per_point;
    }

    pub fn dolly(&mut self, notches: f32) {
        self.distance = (self.distance * DOLLY_STEP.powf(notches)).clamp(
            self.framed_distance * DOLLY_MIN,
            self.framed_distance * DOLLY_MAX,
        );
    }

    pub fn set_up(&mut self, up: UpAxis) {
        self.up = up;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Aabb, SceneCamera, UpAxis};
    use glam::{Vec2, Vec3, Vec4};

    fn bx() -> Aabb {
        Aabb {
            min: Vec3::new(-1.0, -2.0, -3.0),
            max: Vec3::new(4.0, 5.0, 6.0),
        }
    }

    fn ndc(m: glam::Mat4, p: Vec3) -> Vec3 {
        let c = m * Vec4::new(p.x, p.y, p.z, 1.0);
        Vec3::new(c.x / c.w, c.y / c.w, c.z / c.w)
    }

    fn up_vec(up: UpAxis) -> Vec3 {
        match up {
            UpAxis::Y => Vec3::Y,
            UpAxis::Z => Vec3::Z,
        }
    }

    #[test]
    fn framed_keeps_whole_box_in_view() {
        let b = bx();
        for up in [UpAxis::Y, UpAxis::Z] {
            let m = OrbitCamera::framed(&b, up).view_proj(1.0, &b);
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { b.min.x } else { b.max.x },
                    if i & 2 == 0 { b.min.y } else { b.max.y },
                    if i & 4 == 0 { b.min.z } else { b.max.z },
                );
                let n = ndc(m, p);
                assert!(n.x.abs() <= 1.0 && n.y.abs() <= 1.0, "{up:?} {p} -> {n}");
                assert!((0.0..=1.0).contains(&n.z), "{up:?} {p} -> {n}");
            }
        }
    }

    #[test]
    fn framed_keeps_whole_box_in_a_portrait_viewport() {
        let b = bx();
        let m = OrbitCamera::framed(&b, UpAxis::Y).view_proj(0.5, &b);
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
            let n = ndc(m, p);
            assert!(n.x.abs() <= 1.0 && n.y.abs() <= 1.0, "{p} -> {n}");
        }
    }

    #[test]
    fn file_camera_keeps_its_horizontal_view_in_a_square() {
        let b = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        let cam = SceneCamera {
            eye: Vec3::new(4.0, 3.0, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            yfov: 0.6,
            aspect: Some(16.0 / 9.0),
        };
        let o = OrbitCamera::from_scene(&cam, &b, UpAxis::Y);
        let forward = (o.target - o.eye()).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let cam_up = right.cross(forward);
        let half_v = (0.6f32 * 0.5).tan();
        let edge_x = o.eye() + forward * 5.0 + right * 5.0 * half_v * (16.0 / 9.0);
        for aspect in [16.0 / 9.0, 1.0] {
            let n = ndc(o.view_proj(aspect, &b), edge_x);
            assert!((n.x - 1.0).abs() < 1e-3, "aspect {aspect}: {n}");
        }
        // Wider viewports keep the authored vertical FOV.
        let edge_y = o.eye() + forward * 5.0 + cam_up * 5.0 * half_v;
        let n = ndc(o.view_proj(2.5, &b), edge_y);
        assert!((n.y - 1.0).abs() < 1e-3, "{n}");
    }

    #[test]
    fn framed_up_axis_is_world_up() {
        let b = bx();
        for up in [UpAxis::Y, UpAxis::Z] {
            let m = OrbitCamera::framed(&b, up).view_proj(1.0, &b);
            let c = b.center();
            assert!(ndc(m, c + up_vec(up)).y > ndc(m, c).y, "{up:?}");
        }
    }

    #[test]
    fn from_scene_reproduces_file_camera() {
        let cam = SceneCamera {
            eye: Vec3::new(4.0, 3.0, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            yfov: 0.6,
            aspect: None,
        };
        let b = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        let o = OrbitCamera::from_scene(&cam, &b, UpAxis::Y);
        assert!((o.eye() - cam.eye).length() < 1e-4, "{}", o.eye());
        let dir = (o.target - o.eye()).normalize();
        assert!((dir - (-cam.eye).normalize()).length() < 1e-4);
    }

    #[test]
    fn degenerate_inputs_stay_finite() {
        let flat = Aabb {
            min: Vec3::ONE,
            max: Vec3::ONE,
        };
        let cam = SceneCamera {
            eye: Vec3::ONE,
            target: Vec3::ONE,
            up: Vec3::Y,
            yfov: 0.5,
            aspect: None,
        };
        let vertical = SceneCamera {
            eye: Vec3::new(0.0, 5.0, 0.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            yfov: 0.5,
            aspect: None,
        };
        for o in [
            OrbitCamera::framed(&flat, UpAxis::Y),
            OrbitCamera::from_scene(&cam, &flat, UpAxis::Y),
            OrbitCamera::from_scene(&vertical, &bx(), UpAxis::Y),
        ] {
            let m = o.view_proj(1.0, &flat);
            assert!(m.to_cols_array().iter().all(|v| v.is_finite()), "{o:?}");
        }
    }

    #[test]
    fn orbit_clamps_pitch() {
        let mut o = OrbitCamera::framed(&bx(), UpAxis::Y);
        o.orbit(Vec2::new(0.0, 1e6));
        assert!(o.pitch.abs() <= 89f32.to_radians() + 1e-6);
        o.orbit(Vec2::new(0.0, -1e7));
        assert!(o.pitch.abs() <= 89f32.to_radians() + 1e-6);
    }

    #[test]
    fn dolly_in_then_out_is_identity() {
        let mut o = OrbitCamera::framed(&bx(), UpAxis::Y);
        let d = o.distance;
        o.dolly(3.0);
        assert!(o.distance < d);
        o.dolly(-3.0);
        assert!((o.distance - d).abs() < 1e-4);
    }

    #[test]
    fn pan_moves_target_in_view_plane_following_cursor() {
        let b = bx();
        let mut o = OrbitCamera::framed(&b, UpAxis::Y);
        let before = o.view_proj(1.0, &b);
        let t0 = o.target;
        let p0 = ndc(before, t0);
        o.pan(Vec2::new(10.0, 0.0), 100.0);
        // The old target point must now sit further right on screen.
        let p1 = ndc(o.view_proj(1.0, &b), t0);
        assert!(p1.x > p0.x);
        assert!((p1.y - p0.y).abs() < 1e-4);
    }

    #[test]
    fn dolly_is_clamped() {
        let mut o = OrbitCamera::framed(&bx(), UpAxis::Y);
        let d = o.distance;
        o.dolly(1e6);
        assert!((o.distance - d * 1e-3).abs() < d * 1e-6);
        o.dolly(-1e6);
        assert!((o.distance - d * 1e4).abs() < d);
    }
}

#[cfg(test)]
mod yfov_tests {
    use super::*;

    #[test]
    fn invalid_file_yfov_falls_back_to_default() {
        let b = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        for yfov in [0.0, f32::NAN, -1.0, 3.5, f32::INFINITY] {
            let cam = SceneCamera {
                eye: Vec3::new(4.0, 3.0, 4.0),
                target: Vec3::ZERO,
                up: Vec3::Y,
                yfov,
                aspect: None,
            };
            let o = OrbitCamera::from_scene(&cam, &b, UpAxis::Y);
            assert_eq!(o.yfov, 0.8, "{yfov}");
            assert!(o
                .view_proj(1.0, &b)
                .to_cols_array()
                .iter()
                .all(|v| v.is_finite()));
        }
    }
}
