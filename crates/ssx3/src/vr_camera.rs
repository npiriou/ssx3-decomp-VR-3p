//! OpenXR eye poses relative to the game's moving chase camera.

use ssx3_gfx::vr::{self, Eye};
use ssx3_vr::openxr as xr;

#[derive(Clone, Copy)]
pub struct Anchor {
    orientation: xr::Quaternionf,
    midpoint: [f32; 3],
}

fn xyz(p: xr::Vector3f) -> [f32; 3] {
    [p.x, p.y, p.z]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn flip_z(a: [f32; 3]) -> [f32; 3] {
    [a[0], a[1], -a[2]]
}
fn conj(q: xr::Quaternionf) -> xr::Quaternionf {
    xr::Quaternionf {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: q.w,
    }
}
fn mul(a: xr::Quaternionf, b: xr::Quaternionf) -> xr::Quaternionf {
    xr::Quaternionf {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}
fn rotate(q: xr::Quaternionf, v: [f32; 3]) -> [f32; 3] {
    let u = [q.x, q.y, q.z];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = scale(cross(u, v), 2.0);
    add(v, add(scale(t, q.w), cross(u, t)))
}

impl Anchor {
    pub fn new(views: &[xr::View]) -> Option<Self> {
        if views.len() != 2 {
            return None;
        }
        Some(Self {
            orientation: views[0].pose.orientation,
            midpoint: scale(
                add(xyz(views[0].pose.position), xyz(views[1].pose.position)),
                0.5,
            ),
        })
    }

    /// `units_per_metre` makes a physical 64 mm IPD about 6.4 game units
    /// at the default of 100. The source projection continues to track the
    /// rider through the game's camera each update.
    pub fn eyes(self, views: &[xr::View], units_per_metre: f32) -> Option<[Eye; 2]> {
        if views.len() != 2 {
            return None;
        }
        Some(std::array::from_fn(|i| {
            let v = &views[i];
            let baseline_to_eye = mul(conj(self.orientation), v.pose.orientation);
            let eye_to_baseline = conj(baseline_to_eye);
            let offset = rotate(
                conj(self.orientation),
                sub(xyz(v.pose.position), self.midpoint),
            );
            let mut view = vr::IDENTITY;
            for c in 0..3 {
                let mut basis = [0.0; 3];
                basis[c] = 1.0;
                let out = flip_z(rotate(eye_to_baseline, flip_z(basis)));
                view[c] = [out[0], out[1], out[2], 0.0];
            }
            let t = scale(flip_z(rotate(eye_to_baseline, offset)), -units_per_metre);
            view[3] = [t[0], t[1], t[2], 1.0];
            let f = v.fov;
            Eye {
                view_from_game: view,
                projection: vr::projection(
                    f.angle_left.tan(),
                    f.angle_right.tan(),
                    f.angle_down.tan(),
                    f.angle_up.tan(),
                    1.0,
                    30_000.0,
                ),
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_head_preserves_game_forward_and_ipd() {
        let mut left = xr::View {
            pose: xr::Posef::IDENTITY,
            fov: xr::Fovf {
                angle_left: -0.8,
                angle_right: 0.8,
                angle_up: 0.8,
                angle_down: -0.8,
            },
        };
        left.pose.position.x = -0.032;
        let mut right = left;
        right.pose.position.x = 0.032;
        let views = [left, right];
        let eyes = Anchor::new(&views).unwrap().eyes(&views, 100.0).unwrap();
        assert!((eyes[0].view_from_game[3][0] - 3.2).abs() < 1e-4);
        assert!((eyes[1].view_from_game[3][0] + 3.2).abs() < 1e-4);
        assert_eq!(eyes[0].view_from_game[2][2], 1.0);
    }

    #[test]
    fn turning_head_rotates_game_forward_without_moving_chase_anchor() {
        let fov = xr::Fovf {
            angle_left: -0.8,
            angle_right: 0.8,
            angle_up: 0.8,
            angle_down: -0.8,
        };
        let neutral = xr::View {
            pose: xr::Posef::IDENTITY,
            fov,
        };
        let anchor = Anchor::new(&[neutral, neutral]).unwrap();
        let mut turned = neutral;
        turned.pose.orientation = xr::Quaternionf {
            x: 0.0,
            y: std::f32::consts::FRAC_1_SQRT_2,
            z: 0.0,
            w: std::f32::consts::FRAC_1_SQRT_2,
        };
        let eyes = anchor.eyes(&[turned, turned], 100.0).unwrap();
        let forward = eyes[0].view_from_game[2];
        assert!((forward[0] - 1.0).abs() < 1e-4);
        assert!(forward[2].abs() < 1e-4);
        assert_eq!(eyes[0].view_from_game[3], [0.0, 0.0, 0.0, 1.0]);
    }
}
