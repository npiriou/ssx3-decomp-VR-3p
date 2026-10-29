//! Per-eye projection of the game's already built 3D triangles.
//!
//! The game camera still follows the rider. Each eye adds a local head pose
//! to that camera; no game state or EE function is changed here.

#[derive(Clone, Copy, Debug)]
pub struct Eye {
    /// Game-camera coordinates to this eye's coordinates, column major.
    pub view_from_game: [[f32; 4]; 4],
    /// Eye coordinates to clip coordinates, positive Z forward, column major.
    pub projection: [[f32; 4]; 4],
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}

pub fn transform(m: [[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|r| (0..4).map(|c| m[c][r] * v[c]).sum())
}

/// Invert an arbitrary 4x4 projection. Reject singular or invalid frames.
pub fn inverse(m: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    let mut a = [[0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = m[c][r] as f64;
        }
        a[r][4 + r] = 1.0;
    }
    for c in 0..4 {
        let p = (c..4).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        if a[p][c].abs() < 1e-12 || !a[p][c].is_finite() {
            return None;
        }
        a.swap(c, p);
        let scale = a[c][c];
        for k in 0..8 {
            a[c][k] /= scale;
        }
        for r in 0..4 {
            if r != c {
                let factor = a[r][c];
                for k in 0..8 {
                    a[r][k] -= factor * a[c][k];
                }
            }
        }
    }
    Some(std::array::from_fn(|c| {
        std::array::from_fn(|r| a[r][c + 4] as f32)
    }))
}

/// Map the game's homogeneous clip position to one tracked eye's clip
/// position. Both projections use clip Z in [-W, W].
pub fn reproject(eye: Eye, game_projection: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    Some(mul(
        eye.projection,
        mul(eye.view_from_game, inverse(game_projection)?),
    ))
}

/// Positive-Z-forward perspective with OpenXR's asymmetric tangent bounds.
pub fn projection(left: f32, right: f32, down: f32, up: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let sx = 2.0 / (right - left);
    let sy = 2.0 / (up - down);
    let z = (far + near) / (far - near);
    let tz = -2.0 * far * near / (far - near);
    [
        [sx, 0.0, 0.0, 0.0],
        [0.0, sy, 0.0, 0.0],
        [
            -(right + left) / (right - left),
            -(up + down) / (up - down),
            z,
            1.0,
        ],
        [0.0, 0.0, tz, 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_eye_is_identity() {
        let p = projection(-0.9, 1.1, -1.0, 0.8, 1.0, 30_000.0);
        let m = reproject(
            Eye {
                view_from_game: IDENTITY,
                projection: p,
            },
            p,
        )
        .unwrap();
        for c in 0..4 {
            for r in 0..4 {
                assert!((m[c][r] - IDENTITY[c][r]).abs() < 1e-3, "{c},{r}");
            }
        }
    }

    #[test]
    fn eye_offset_changes_near_points_more_than_far_points() {
        let p = projection(-1.0, 1.0, -1.0, 1.0, 1.0, 30_000.0);
        let mut view = IDENTITY;
        view[3][0] = -3.2;
        let m = reproject(
            Eye {
                view_from_game: view,
                projection: p,
            },
            p,
        )
        .unwrap();
        let x = |z: f32| {
            let game = transform(p, [0.0, 0.0, z, 1.0]);
            let eye = transform(m, game);
            eye[0] / eye[3]
        };
        assert!(x(100.0).abs() > x(1000.0).abs() * 9.0);
    }
}
