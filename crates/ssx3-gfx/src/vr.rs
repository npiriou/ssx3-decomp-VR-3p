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

/// Background art is at infinity: retain orientation and each eye's FOV,
/// while removing the finite-depth disparity of the game's tiny sky mesh.
pub fn sky_reproject(mut eye: Eye, game_projection: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    eye.view_from_game[3][..3].fill(0.0);
    reproject(eye, game_projection)
}

/// Native horizon edges identify a mesh instance, even when several share
/// a texture. Artwork emission order is not angular order.
pub fn sky_sector_angles(call: &ssx3_render::gpu::GpuCall) -> Option<[f32; 2]> {
    let mut edges = [None, None];
    for v in call.data.chunks_exact(4) {
        let p = v[0].map(|x| x as i32 as f32 / 32768.0);
        if !(-0.40..=-0.38).contains(&p[2]) || p[0].hypot(p[1]) < 0.6 {
            continue;
        }
        if v[1][0] == 4096 {
            edges[0] = Some(p[1].atan2(p[0]));
        }
        if v[1][0] == 0 {
            edges[1] = Some(p[1].atan2(p[0]));
        }
    }
    Some([edges[0]?, edges[1]?])
}

fn angle_distance(a: f32, b: f32) -> f32 {
    ((a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs()
}

/// Orient a culled upper strip with its actual lower partner. The fourth
/// native panel may precede or follow the other three, depending on the sky.
pub fn missing_sky_sector(
    sources: &[(u16, ssx3_render::gpu::GpuCall)],
    texture: u16,
    layout: ssx3_render::sky::Layout,
) -> Option<ssx3_render::gpu::GpuCall> {
    let (slot, upper) = layout.sector(texture)?;
    let anchor = sources.iter().find_map(|(id, call)| {
        if layout.sector(*id)?.0 != 0 {
            return None;
        }
        sky_sector_angles(call).map(|a| a[0])
    });
    let expected = anchor.map(|start| start + slot as f32 * std::f32::consts::FRAC_PI_4);
    let paired = sources
        .iter()
        .filter(|(id, call)| {
            layout.sector(*id) == Some((slot, !upper)) && sky_sector_angles(call).is_some()
        })
        .min_by(|a, b| {
            let error = |entry: &(u16, ssx3_render::gpu::GpuCall)| {
                expected.map_or(0.0, |angle| {
                    angle_distance(sky_sector_angles(&entry.1).unwrap()[0], angle)
                })
            };
            error(a).total_cmp(&error(b))
        });
    let (source_texture, source) = paired.or_else(|| sources.last())?;
    native_sky_sector(source, *source_texture, texture, layout)
}

#[derive(Clone, Copy)]
pub struct SkyPanel {
    pub data_start: usize,
    pub angles: [f32; 2],
    pub upper: bool,
}

/// Join actual neighbouring instances opaquely and fade only open edges.
/// Each reconstructed panel has 17 rows, 9 columns, 4 qwords per vertex.
pub fn stitch_sky_panels(data: &mut [[u32; 4]], panels: &[SkyPanel]) {
    for panel in panels {
        let open_start = !panels.iter().any(|other| {
            other.upper == panel.upper && angle_distance(other.angles[1], panel.angles[0]) < 0.01
        });
        let open_end = !panels.iter().any(|other| {
            other.upper == panel.upper && angle_distance(other.angles[0], panel.angles[1]) < 0.01
        });
        for row in 0..=16 {
            let t = row as f32 / 16.;
            let vertical = if panel.upper {
                (t / 0.125).clamp(0., 1.)
            } else {
                ((1. - t) / 0.25).clamp(0., 1.)
            };
            for col in 0..=8 {
                let s = col as f32 / 8.;
                let edge = (if open_start { s / 0.5 } else { 1. })
                    .min(if open_end { (1. - s) / 0.5 } else { 1. })
                    .clamp(0., 1.);
                if let Some(color) = data.get_mut(panel.data_start + (row * 9 + col) * 4 + 2) {
                    color[3] = (128. * vertical * edge).round() as u32;
                }
            }
        }
    }
}

/// VU1 omits static mesh inputs on later frames once the GS arena holds
/// them. VR sky reconstruction still needs the original vertices, so recover
/// a whole cached Mat3 mesh before remapping it around the tracked view.
pub fn restore_cached_sky_mesh(
    call: &mut ssx3_render::gpu::GpuCall,
    arena: &crate::gs::MeshArena,
) -> bool {
    if !call.data.is_empty() {
        return true;
    }
    if call.program != ssx3_render::gpu::Program::Mat3Mesh || call.split.is_some() {
        return false;
    }
    let Some((start, _)) = call.mesh_key.and_then(|key| arena.at.get(&key)) else {
        return false;
    };
    let count = call.uniforms[4][1] as usize;
    if count == 0 || count > 4096 {
        return false;
    }
    let start = *start as usize;
    let Some(end) = count
        .checked_mul(4)
        .and_then(|size| start.checked_add(size))
    else {
        return false;
    };
    let Some(data) = arena.data.get(start..end) else {
        return false;
    };
    call.data.extend_from_slice(data);
    true
}

/// A culled sector keeps its artwork but must follow this frame's sky
/// transform. Mixing old and current screen matrices splits shared edges.
pub fn rebase_cached_sky_panel(
    cached: &mut ssx3_render::gpu::GpuCall,
    current: &ssx3_render::gpu::GpuCall,
) {
    cached.uniforms[..4].copy_from_slice(&current.uniforms[..4]);
    cached.uniforms[18] = current.uniforms[18];
}

/// Reconstruct an unseen Snow Jam sector from its native 45-degree layout.
/// Texture IDs are the original paired lower/upper artwork, never a repeated
/// neighbouring image. The caller selects that texture on the resulting batch.
pub fn snow_sky_sector(
    template: &ssx3_render::gpu::GpuCall,
    template_texture: u16,
    texture: u16,
) -> Option<ssx3_render::gpu::GpuCall> {
    native_sky_sector(
        template,
        template_texture,
        texture,
        ssx3_render::sky::Layout { base: 281 },
    )
}

pub fn native_sky_sector(
    template: &ssx3_render::gpu::GpuCall,
    template_texture: u16,
    texture: u16,
    layout: ssx3_render::sky::Layout,
) -> Option<ssx3_render::gpu::GpuCall> {
    use ssx3_render::gpu::{Link, Program};
    let (index, upper) = layout.sector(texture)?;
    let (source_index, _) = layout.sector(template_texture)?;
    if template.program != Program::Mat3Mesh {
        return None;
    }
    let start = template.data.chunks_exact(4).find_map(|v| {
        let p = v[0].map(|x| x as i32 as f32 / 32768.0);
        (v[1][0] == 4096 && (-0.40..=-0.38).contains(&p[2])).then(|| p[1].atan2(p[0]))
    })? + ((index as i32 - source_index as i32) as f32 * 45.0).to_radians();
    let vertex = template.data.chunks_exact(4).next()?;
    let mut out = template.clone();
    out.link = Link::None;
    out.split = None;
    out.mesh_key = None;
    out.data.clear();
    out.indices.clear();
    out.index_mode = 0;
    // Only the sector's angular limits enter sky_native_panorama_panels;
    // it reconstructs the native lower rings and upper strips consistently.
    for col in 0..3 {
        let angle = start + (col as f32 * 22.5).to_radians();
        let (sn, co) = angle.sin_cos();
        for (z, v) in if upper {
            [(-0.3923645, 4096), (0.3923645, 0)]
        } else {
            [(-0.3923645, 0), (-0.75457764, 2048)]
        } {
            let mut record = [vertex[0], vertex[1], vertex[2], vertex[3]];
            record[0] = [0.6340332 * co, 0.6340332 * sn, z, 0.0]
                .map(|p| ((p * 32768.0).round() as i32) as u32);
            record[1][0] = 4096 - col * 2048;
            record[1][1] = v;
            out.data.extend(record);
        }
    }
    if !upper {
        // The lower-sector discriminator is 15/16 native vertices.
        let first = out.data[..4].to_vec();
        while out.data.len() < 16 * 4 {
            out.data.extend_from_slice(&first);
        }
    }
    out.uniforms[4][1] = (out.data.len() / 4) as u32;
    Some(out)
}

/// Extend the original sky's blue dome below its artist-authored horizon.
/// SSX 3 only needed its narrow chase-camera view: the native blue dome
/// stops above the mountain skirt, leaving a band exposed by a sideways VR
/// view. This makes real VU1 mesh triangles from the dome's own bottom ring,
/// with its vertex colours retained and the original sky texture blended
/// toward its centre below the horizon.
/// The original mountain and course geometry still draw over it.
pub fn sky_dome_lower_band(
    calls: &[ssx3_render::gpu::GpuCall],
) -> Option<ssx3_render::gpu::GpuCall> {
    use ssx3_render::gpu::Link;
    let base = calls.first()?;
    if base.program != ssx3_render::gpu::Program::Mat3Mesh {
        return None;
    }
    let mut source = Vec::new();
    for call in calls {
        for vertex in call.data.chunks_exact(4) {
            let p = [
                vertex[0][0] as i32 as f32 / 32768.0,
                vertex[0][1] as i32 as f32 / 32768.0,
                vertex[0][2] as i32 as f32 / 32768.0,
            ];
            source.push((p, [vertex[0], vertex[1], vertex[2], vertex[3]]));
        }
    }
    let lowest = source.iter().map(|(p, _)| p[2]).reduce(f32::min)?;
    if !(-0.8..=-0.1).contains(&lowest) {
        return None;
    }
    let mut ring = Vec::new();
    for (p, vertex) in source {
        if (p[2] - lowest).abs() > 0.002 {
            continue;
        }
        if ring.iter().any(|(q, _): &([f32; 3], [[u32; 4]; 4])| {
            (p[0] - q[0]).abs() < 0.001 && (p[1] - q[1]).abs() < 0.001
        }) {
            continue;
        }
        ring.push((p, vertex));
    }
    ring.sort_by(|a, b| a.0[1].atan2(a.0[0]).total_cmp(&b.0[1].atan2(b.0[0])));
    if ring.len() < 12 || ring.len() > 64 {
        return None;
    }
    let angles: Vec<_> = ring.iter().map(|(p, _)| p[1].atan2(p[0])).collect();
    for i in 0..ring.len() {
        let next = if i + 1 == ring.len() {
            angles[0] + std::f32::consts::TAU
        } else {
            angles[i + 1]
        };
        if next - angles[i] > 0.7 {
            return None;
        }
    }
    let n = ring.len() as u32;
    let mut out = base.clone();
    out.pre.clear();
    out.data.clear();
    out.indices.clear();
    out.index_mode = 0;
    out.mesh_key = None;
    out.split = None;
    out.link = Link::None;
    let put = |mut vertex: [[u32; 4]; 4], p: [f32; 3], out: &mut ssx3_render::gpu::GpuCall| {
        for j in 0..3 {
            vertex[0][j] = ((p[j] * 32768.0).round() as i32) as u32;
        }
        out.data.extend_from_slice(&vertex);
    };
    for (p, vertex) in &ring {
        put(*vertex, *p, &mut out);
    }
    for (p, vertex) in &ring {
        let radius = (p[0] * p[0] + p[1] * p[1]).sqrt().max(1e-6);
        let new_radius = (1.0f32 - 0.75 * 0.75).sqrt();
        let mut lower = *vertex;
        lower[1][0] = ((lower[1][0] as i32 + 2048) / 2) as u32;
        lower[1][1] = ((lower[1][1] as i32 + 2048) / 2) as u32;
        put(
            lower,
            [
                p[0] * new_radius / radius,
                p[1] * new_radius / radius,
                -0.75,
            ],
            &mut out,
        );
    }
    for (_, vertex) in &ring {
        let mut pole = *vertex;
        pole[1][0] = 2048;
        pole[1][1] = 2048;
        put(pole, [0.0, 0.0, -1.0], &mut out);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        out.indices
            .extend_from_slice(&[i, j, n + i, j, n + j, n + i]);
        out.indices.extend_from_slice(&[n + i, n + j, 2 * n + i]);
    }
    out.uniforms[4][1] = 3 * n;
    Some(out)
}

/// Continuous VR blue sky from the original polar texture and GS material.
/// This replaces the four coarse native dome meshes for tracked views.
pub fn sky_dome_background(
    calls: &[ssx3_render::gpu::GpuCall],
    clean_polar: bool,
) -> Option<ssx3_render::gpu::GpuCall> {
    use ssx3_render::gpu::{Link, Program};
    let base = calls.first()?;
    if calls.len() != 4
        || calls.iter().any(|c| {
            c.program != Program::Mat3Mesh
                || c.uniforms[4][0] != 0
                || c.data.len() != c.uniforms[4][1] as usize * 4
        })
    {
        return None;
    }
    let top = calls
        .iter()
        .flat_map(|c| c.data.chunks_exact(4))
        .map(|v| v[0][2] as i32 as f32 / 32768.0)
        .reduce(f32::max)?;
    let radius = calls
        .iter()
        .flat_map(|c| c.data.chunks_exact(4))
        .map(|v| {
            let x = v[0][0] as i32 as f32 / 32768.0;
            let y = v[0][1] as i32 as f32 / 32768.0;
            x.hypot(y)
        })
        .reduce(f32::max)?;
    if !(0.4..=1.2).contains(&top) || !(0.8..=1.2).contains(&radius) {
        return None;
    }
    const AROUND: u32 = 64;
    const VERTICAL: u32 = 32;
    let mut out = base.clone();
    out.data.clear();
    out.indices.clear();
    out.index_mode = 0;
    out.link = Link::None;
    out.split = None;
    out.mesh_key = None;
    out.uniforms[4][3] = 2;
    let template = [base.data[0], base.data[1], base.data[2], base.data[3]];
    for row in 0..=VERTICAL {
        let latitude =
            -std::f32::consts::FRAC_PI_2 + row as f32 * std::f32::consts::PI / VERTICAL as f32;
        let (height, width) = latitude.sin_cos();
        for col in 0..=AROUND {
            let angle = col as f32 * std::f32::consts::TAU / AROUND as f32;
            let (s, c) = angle.sin_cos();
            let x = width * c;
            let y = width * s;
            let mut v = template;
            let r = radius * 1.04;
            for (lane, coord) in [r * x, r * y, top - radius + r * height]
                .into_iter()
                .enumerate()
            {
                v[0][lane] = ((coord * 32768.0).round() as i32) as u32;
            }
            // The native outer ring contains a sharp circular transition
            // that becomes a giant polygon in a wide tracked view. Sample
            // the original texture's interior blue/cloud region instead.
            let (u, tex_v) = if clean_polar {
                // The source polar map has a thin diagonal pale line across
                // its centre. A complete VR sky enlarges that mark into a
                // bright streak behind the rider. Sample its intact upper
                // blue/cloud region while keeping the original texture.
                (0.5 + 0.20 * x, 0.25 - 0.16 * y)
            } else {
                (0.5 + 0.32 * x, 0.5 - 0.32 * y)
            };
            v[1][0] = (u * 4096.0).round().clamp(0.0, 4095.0) as u32;
            v[1][1] = (tex_v * 4096.0).round().clamp(0.0, 4095.0) as u32;
            out.data.extend_from_slice(&v);
        }
    }
    for row in 0..VERTICAL {
        for col in 0..AROUND {
            let a = row * (AROUND + 1) + col;
            let b = a + 1;
            let c = a + AROUND + 1;
            out.indices.extend_from_slice(&[a, b, c, b, c + 1, c]);
        }
    }
    out.uniforms[4][1] = (out.data.len() / 4) as u32;
    Some(out)
}

/// Place a native 45-degree mountain texture on a camera-space sky shell.
/// The PS2's six flat panels omit 90 degrees behind the chase camera. Reuse
/// their textures and GS materials, and mirror the two adjacent panels into
/// that missing sector. The shell is independent of the old sky screen matrix.
pub fn sky_panorama_panels(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
) -> Option<Vec<ssx3_render::gpu::GpuCall>> {
    use ssx3_render::gpu::{Link, Program};
    if call.program != Program::Mat3Mesh
        || call.uniforms[4][0] != 0
        || !(15..=16).contains(&(call.data.len() / 4))
        || call.data.len() != call.uniforms[4][1] as usize * 4
    {
        return None;
    }
    let mut ring = [(0.0f32, 0.0f32, 0usize); 3];
    let mut start = None;
    let mut end = None;
    for v in call.data.chunks_exact(4) {
        let x = v[0][0] as i32 as f32 / 32768.0;
        let y = v[0][1] as i32 as f32 / 32768.0;
        let z = v[0][2] as i32 as f32 / 32768.0;
        let u = v[1][0] as i32 as f32 / 4096.0;
        let tex_v = v[1][1] as i32 as f32 / 4096.0;
        let r = x.hypot(y);
        let row = if tex_v.abs() < 0.01 {
            0
        } else if (tex_v - 0.5).abs() < 0.01 {
            1
        } else if (tex_v - 1.0).abs() < 0.01 {
            2
        } else {
            return None;
        };
        ring[row].0 += r;
        ring[row].1 += z;
        ring[row].2 += 1;
        if row == 0 && u > 0.99 {
            start = Some(y.atan2(x));
        } else if row == 0 && u < 0.01 {
            end = Some(y.atan2(x));
        }
    }
    if ring.iter().any(|r| r.2 < 3) {
        return None;
    }
    let rings = ring.map(|(r, z, n)| (r / n as f32, z / n as f32));
    if !(0.60..=0.68).contains(&rings[0].0)
        || !(-0.43..=-0.35).contains(&rings[0].1)
        || !(-0.92..=-0.83).contains(&rings[2].1)
    {
        return None;
    }
    let start = start?;
    let mut end = end?;
    if end <= start {
        end += std::f32::consts::TAU;
    }
    let width = end - start;
    if !(0.72..=0.85).contains(&width) {
        return None;
    }
    if game_projection[2][3].abs() <= 0.5
        || game_projection.iter().flatten().any(|v| !v.is_finite())
    {
        return None;
    }
    const HORIZONTAL: u32 = 8;
    const VERTICAL: u32 = 24;
    const RADIUS: f32 = 10_000.0;
    let template = call.data.chunks_exact(4).next()?;
    let panel = |angle_start: f32, flip_u: bool| {
        let mut out = call.clone();
        out.pre.clear();
        out.data.clear();
        out.indices.clear();
        out.index_mode = 0;
        out.link = Link::None;
        out.split = None;
        out.mesh_key = None;
        out.uniforms[4][2] = 1;
        out.uniforms[4][3] = 3;
        for (q, column) in game_projection.into_iter().enumerate() {
            out.uniforms[q] = column.map(f32::to_bits);
        }
        for row in 0..=VERTICAL {
            let t = row as f32 / VERTICAL as f32;
            // The shell covers both poles; the real course hides its base.
            let elevation = if t <= 0.5 {
                89.0 - 120.0 * t
            } else if t <= 0.75 {
                29.0 - 160.0 * (t - 0.5)
            } else {
                -11.0 - 312.0 * (t - 0.75)
            }
            .to_radians();
            let (height, width_at_elevation) = elevation.sin_cos();
            for col in 0..=HORIZONTAL {
                let s = col as f32 / HORIZONTAL as f32;
                let yaw = angle_start + width * s - std::f32::consts::PI;
                let (sn, cs) = yaw.sin_cos();
                let mut vertex = [template[0], template[1], template[2], template[3]];
                for (lane, p) in [
                    RADIUS * width_at_elevation * sn,
                    -RADIUS * height,
                    RADIUS * width_at_elevation * cs,
                ]
                .into_iter()
                .enumerate()
                {
                    vertex[0][lane] = ((p * 32768.0).round() as i32) as u32;
                }
                let u = if flip_u { s } else { 1.0 - s };
                vertex[1][0] = (u * 4096.0).round() as u32;
                // The GS texture is vertically inverted relative to the
                // extracted PNG. The visible upper half of this shell spans
                // a small part of `t`; map the panorama's sky-to-mountain
                // transition into that range.
                let tex_v = (5.0 * (t - 0.65)).clamp(0.0, 1.0);
                vertex[1][1] = (tex_v * 4096.0).round() as u32;
                let high_sky = ((0.84 - t) / 0.08).clamp(0.0, 1.0);
                let towards_back = if flip_u {
                    if angle_start < start {
                        1.0 - s
                    } else {
                        s
                    }
                } else {
                    0.0
                };
                let back_fade = 1.0 - towards_back * towards_back;
                vertex[2][3] = (128.0 * high_sky * back_fade).round() as u32;
                out.data.extend_from_slice(&vertex);
            }
        }
        for row in 0..VERTICAL {
            for col in 0..HORIZONTAL {
                let a = row * (HORIZONTAL + 1) + col;
                let b = a + 1;
                let c = a + HORIZONTAL + 1;
                out.indices.extend_from_slice(&[a, b, c, b, c + 1, c]);
            }
        }
        out.uniforms[4][1] = (out.data.len() / 4) as u32;
        Some(out)
    };
    let mut panels = vec![panel(start, false)?];
    panels[0].pre = call.pre.clone();
    let deg = start.to_degrees();
    if (deg - 45.0).abs() < 1.0 {
        panels.push(panel(start - width, true)?);
    } else if (deg + 90.0).abs() < 1.0 {
        panels.push(panel(start + width, true)?);
    }
    Some(panels)
}

/// Rebuild the native sky's lower sectors and six-vertex upper strips on a
/// camera-space sphere. The original 45-degree sectors and texture coordinates
/// determine each panel's placement, for courses with this sky mesh layout.
pub fn sky_native_panorama_panels(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
    fade_upper_edge: bool,
    join_gravitude_upper: bool,
    blend_gravitude_horizon: bool,
    fade_all_side_edges: bool,
) -> Option<Vec<ssx3_render::gpu::GpuCall>> {
    sky_native_panorama_panels_impl(
        call,
        game_projection,
        xyoffset,
        gs_size,
        fade_upper_edge,
        join_gravitude_upper,
        blend_gravitude_horizon,
        fade_all_side_edges,
        None,
    )
}

/// The outer panorama edges follow the artwork's sector index, not a fixed
/// world angle: different native sky meshes rotate this four-sector layout.
pub fn snow_sky_panorama_panels(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
    texture: u16,
) -> Option<Vec<ssx3_render::gpu::GpuCall>> {
    native_sky_panorama_panels(
        call,
        game_projection,
        xyoffset,
        gs_size,
        texture,
        ssx3_render::sky::Layout { base: 281 },
    )
}

pub fn native_sky_panorama_panels(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
    texture: u16,
    layout: ssx3_render::sky::Layout,
) -> Option<Vec<ssx3_render::gpu::GpuCall>> {
    let (sector, _) = layout.sector(texture)?;
    sky_native_panorama_panels_impl(
        call,
        game_projection,
        xyoffset,
        gs_size,
        false,
        false,
        false,
        false,
        Some(sector),
    )
}

fn sky_native_panorama_panels_impl(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
    fade_upper_edge: bool,
    join_gravitude_upper: bool,
    blend_gravitude_horizon: bool,
    fade_all_side_edges: bool,
    snow_sector: Option<usize>,
) -> Option<Vec<ssx3_render::gpu::GpuCall>> {
    use ssx3_render::gpu::{Link, Program};
    if call.program != Program::Mat3Mesh
        || call.uniforms[4][0] != 0
        || !matches!(call.data.len() / 4, 6 | 15 | 16)
        || call.data.len() != call.uniforms[4][1] as usize * 4
        || game_projection[2][3].abs() <= 0.5
        || gs_size.0 == 0
        || gs_size.1 == 0
    {
        return None;
    }
    let mut start = None;
    let mut end = None;
    for v in call.data.chunks_exact(4) {
        let p = v[0].map(|bits| bits as i32 as f32 / 32768.0);
        let u = v[1][0] as f32 / 4096.0;
        if (-0.40..=-0.38).contains(&p[2]) && p[0].hypot(p[1]) > 0.60 && u > 0.99 {
            start = Some(p[1].atan2(p[0]));
        } else if (-0.40..=-0.38).contains(&p[2]) && p[0].hypot(p[1]) > 0.60 && u < 0.01 {
            end = Some(p[1].atan2(p[0]));
        }
    }
    let start = start?;
    let mut end = end?;
    if end <= start {
        end += std::f32::consts::TAU;
    }
    let width = end - start;
    if !(0.72..=0.85).contains(&width) {
        return None;
    }
    let mut out = call.clone();
    out.link = Link::None;
    out.split = None;
    out.mesh_key = None;
    out.data.clear();
    out.indices.clear();
    out.index_mode = 0;
    out.uniforms[4][2] = 1;
    const HORIZONTAL: u32 = 8;
    const VERTICAL: u32 = 16;
    let upper = call.data.len() == 6 * 4;
    let smooth_gravitude = upper && join_gravitude_upper;
    // These source upper strips cover [-90°, 45°] in three contiguous
    // sectors. At their two shared boundaries, fading each strip to the
    // plain blue dome makes a dark vertical notch. Their outer boundaries
    // still need the fade because the remaining directions have no strips.
    let shared_edge = |angle: f32| {
        let angle =
            (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        angle.abs() < 0.02 || (angle + std::f32::consts::FRAC_PI_4).abs() < 0.02
    };
    let keep_start = smooth_gravitude && shared_edge(start);
    let keep_end = smooth_gravitude && shared_edge(end);
    // Snow Jam has native mountain art only from +90° through -90°.
    // Its neighbouring 45° sectors must meet opaquely; blend only the two
    // outer boundaries into the polar blue dome.
    let wrap_angle = |angle: f32| {
        (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
    };
    let fade_start = if let Some(sector) = snow_sector {
        sector == 0
    } else if fade_all_side_edges {
        !keep_start
    } else {
        wrap_angle(start - std::f32::consts::FRAC_PI_2).abs() < 0.02
    };
    let fade_end = if let Some(sector) = snow_sector {
        sector == 3
    } else if fade_all_side_edges {
        !keep_end
    } else {
        wrap_angle(end + std::f32::consts::FRAC_PI_2).abs() < 0.02
    };
    let side_fade_width = if fade_all_side_edges { 0.25 } else { 0.5 };
    let blend_horizon =
        join_gravitude_upper && blend_gravitude_horizon && (shared_edge(start) || shared_edge(end));
    // Put the lower source behind the last two rows of the upper source.
    // Its full V range is mapped across the extended height; repeating one
    // edge texel here produces long vertical streaks in the mountains.
    const OVERLAP_ROWS: u32 = 2;
    let extra_top = if blend_horizon && !upper {
        OVERLAP_ROWS
    } else {
        0
    };
    let rows = VERTICAL + extra_top;
    let template = call.data.chunks_exact(4).next()?;
    for row in 0..=rows {
        let t = (row as i32 - extra_top as i32) as f32 / VERTICAL as f32;
        let (radius, height) = if upper {
            (0.634, 0.392 - 0.784 * t)
        } else if t <= 0.5 {
            (0.634 - 0.524 * t, -0.392 - 0.726 * t)
        } else {
            (0.372 - 0.352 * (t - 0.5), -0.755 - 0.248 * (t - 0.5))
        };
        for col in 0..=HORIZONTAL {
            let s = col as f32 / HORIZONTAL as f32;
            let (sn, co) = (start + width * s).sin_cos();
            let mut vertex = [template[0], template[1], template[2], template[3]];
            for (lane, p) in [radius * co, radius * sn, height].into_iter().enumerate() {
                vertex[0][lane] = ((p * 32768.0).round() as i32) as u32;
            }
            vertex[1][0] = ((1.0 - s) * 4096.0).round() as u32;
            let tex_v = if extra_top != 0 {
                (t + OVERLAP_ROWS as f32 / VERTICAL as f32)
                    / (1.0 + OVERLAP_ROWS as f32 / VERTICAL as f32)
            } else {
                t
            };
            vertex[1][1] = (tex_v * 4096.0).round() as u32;
            let edge = (if fade_start { s / side_fade_width } else { 1.0 })
                .min(if fade_end {
                    (1.0 - s) / side_fade_width
                } else {
                    1.0
                })
                .clamp(0.0, 1.0);
            let vertical = if upper {
                if !fade_all_side_edges {
                    // Snow Jam's upper textures contain their own blue sky.
                    // Blend their open top boundary into the polar dome;
                    // opaque top edges become giant diagonal cuts in VR.
                    (t / 0.125).clamp(0.0, 1.0)
                } else if blend_horizon {
                    ((1.0 - t) * VERTICAL as f32 / OVERLAP_ROWS as f32).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            } else if fade_upper_edge {
                (t / 0.125).min((1.0 - t) / 0.25).clamp(0.0, 1.0)
            } else {
                ((1.0 - t) / 0.25).clamp(0.0, 1.0)
            };
            vertex[2][3] = (128.0 * edge * vertical).round() as u32;
            out.data.extend_from_slice(&vertex);
        }
    }
    for row in 0..rows {
        for col in 0..HORIZONTAL {
            let a = row * (HORIZONTAL + 1) + col;
            let b = a + 1;
            let c = a + HORIZONTAL + 1;
            out.indices.extend_from_slice(&[a, b, c, b, c + 1, c]);
        }
    }
    out.uniforms[4][1] = (out.data.len() / 4) as u32;
    Some(vec![sky_wrap_panel_mesh(
        &out,
        game_projection,
        xyoffset,
        gs_size,
    )?])
}

/// Project every vertex of an original mountain panel back into the game's
/// camera, then move it along that ray onto a distant sphere. This preserves
/// each course's own texture placement and alpha, including the six-vertex
/// overlay strips. The native planar sky can cross a tracked eye's view as a
/// giant triangle; its camera rays carry the intended artwork without that
/// plane's finite depth.
pub fn sky_wrap_native_panel(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
) -> Option<ssx3_render::gpu::GpuCall> {
    use ssx3_render::gpu::Program;
    if call.program != Program::Mat3Mesh
        || call.uniforms[4][0] != 0
        || !matches!(call.data.len() / 4, 6 | 15 | 16)
        || call.data.len() != call.uniforms[4][1] as usize * 4
        || call.indices.is_empty()
        || gs_size.0 == 0
        || gs_size.1 == 0
    {
        return None;
    }
    sky_wrap_panel_mesh(call, game_projection, xyoffset, gs_size)
}

fn sky_wrap_panel_mesh(
    call: &ssx3_render::gpu::GpuCall,
    game_projection: [[f32; 4]; 4],
    xyoffset: [f32; 2],
    gs_size: (u32, u32),
) -> Option<ssx3_render::gpu::GpuCall> {
    use ssx3_render::gpu::Link;
    let inv = inverse(game_projection)?;
    let screen: [[f32; 4]; 4] = std::array::from_fn(|i| call.uniforms[i].map(f32::from_bits));
    let guard = call.uniforms[18].map(f32::from_bits);
    if !guard[0].is_finite() || guard[0].abs() < 1e-10 {
        return None;
    }
    let mut out = call.clone();
    out.link = Link::None;
    out.split = None;
    out.mesh_key = None;
    out.uniforms[4][3] = 3;
    for (column, values) in game_projection.into_iter().enumerate() {
        out.uniforms[column] = values.map(f32::to_bits);
    }
    const RADIUS: f32 = 10_000.0;
    for vertex in out.data.chunks_exact_mut(4) {
        let p = vertex[0].map(|x| x as i32 as f32 / 32768.0);
        let hc = transform(screen, [p[0], p[1], p[2], 1.0]);
        let w = hc[3];
        let px = hc[0] - xyoffset[0] * w;
        let py = hc[1] - xyoffset[1] * w;
        let clip = [
            (px + 0.5 * w) * (2.0 / gs_size.0 as f32) - w,
            w - (py + 0.5 * w) * (2.0 / gs_size.1 as f32),
            (hc[2] - guard[1] * w) / guard[0],
            w,
        ];
        let camera = transform(inv, clip);
        let direction = [camera[0], camera[1], camera[2]];
        let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !length.is_finite() || length < 1e-6 {
            return None;
        }
        for (lane, value) in direction.into_iter().enumerate() {
            vertex[0][lane] = ((RADIUS * value / length * 32768.0).round() as i32) as u32;
        }
    }
    Some(out)
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
    fn sky_joins_follow_mesh_edges_for_both_fourth_panel_orientations() {
        const WORDS: usize = 17 * 9 * 4;
        for angles in [[-45f32, 0., 45., 90.], [-45., 0., 45., -90.]] {
            for rotation in [0.0, 2.7] {
                let mut data = vec![[42; 4]; WORDS * 4];
                let panels: Vec<_> = angles
                    .iter()
                    .enumerate()
                    .map(|(i, a)| SkyPanel {
                        data_start: i * WORDS,
                        angles: [a.to_radians() + rotation, (a + 45.).to_radians() + rotation],
                        upper: true,
                    })
                    .collect();
                stitch_sky_panels(&mut data, &panels);
                for (i, a) in angles.iter().enumerate() {
                    let alpha = |col: usize| data[i * WORDS + (8 * 9 + col) * 4 + 2][3];
                    let left_joined = angles.iter().any(|b| (b + 45. - a).abs() < 0.01);
                    let right_joined = angles.iter().any(|b| (a + 45. - b).abs() < 0.01);
                    assert_eq!(alpha(0), if left_joined { 128 } else { 0 });
                    assert_eq!(alpha(8), if right_joined { 128 } else { 0 });
                    assert_eq!(alpha(4), 128);
                    assert_eq!(data[i * WORDS][0], 42);
                }
            }
        }
    }

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

    #[test]
    fn cached_sky_mesh_restores_the_original_vertex_records() {
        use ssx3_render::gpu::{GpuCall, Link, Program, UNIFORM_QWORDS};
        let source: Vec<[u32; 4]> = (1..=8).map(|value| [value; 4]).collect();
        let mut arena = crate::gs::MeshArena::default();
        arena.data.push([99; 4]);
        arena.data.extend_from_slice(&source);
        arena.at.insert(7, (1, 128.0));
        let mut call = GpuCall {
            program: Program::Mat3Mesh,
            pre: Vec::new(),
            tag: 0,
            uniforms: [[0; 4]; UNIFORM_QWORDS],
            data: Vec::new(),
            indices: Vec::new(),
            index_mode: 0,
            alpha_max: 128.0,
            link: Link::None,
            split: None,
            mesh_key: Some(7),
        };
        call.uniforms[4][1] = 2;
        assert!(restore_cached_sky_mesh(&mut call, &arena));
        assert_eq!(call.data, source);
        call.data.clear();
        call.mesh_key = Some(8);
        assert!(!restore_cached_sky_mesh(&mut call, &arena));
        assert!(call.data.is_empty());
    }

    #[test]
    fn sky_lower_band_requires_a_closed_native_ring() {
        use ssx3_render::gpu::{GpuCall, Link, Program, UNIFORM_QWORDS};
        let mut call = GpuCall {
            program: Program::Mat3Mesh,
            pre: Vec::new(),
            tag: 0,
            uniforms: [[0; 4]; UNIFORM_QWORDS],
            data: Vec::new(),
            indices: Vec::new(),
            index_mode: 0,
            alpha_max: 128.0,
            link: Link::None,
            split: None,
            mesh_key: None,
        };
        for i in 0..16 {
            let a = i as f32 * std::f32::consts::TAU / 16.0;
            let (s, c) = a.sin_cos();
            call.data.extend_from_slice(&[
                [
                    ((c * 32768.0) as i32) as u32,
                    ((s * 32768.0) as i32) as u32,
                    ((-0.3 * 32768.0) as i32) as u32,
                    0,
                ],
                [2048, 2048, 0, 0],
                [128, 128, 128, 128],
                [0; 4],
            ]);
        }
        let filled = sky_dome_lower_band(&[call.clone()]).expect("complete ring");
        assert_eq!(filled.data.len(), 16 * 3 * 4);
        assert_eq!(filled.indices.len(), 16 * 9);
        call.data.truncate(8 * 4);
        assert!(sky_dome_lower_band(&[call]).is_none());
    }

    #[test]
    fn sky_panorama_mirrors_native_edges_into_the_missing_sector() {
        use ssx3_render::gpu::{GpuCall, Link, Program, UNIFORM_QWORDS};
        let make = |start_deg: f32| {
            let mut call = GpuCall {
                program: Program::Mat3Mesh,
                pre: Vec::new(),
                tag: 0,
                uniforms: [[0; 4]; UNIFORM_QWORDS],
                data: Vec::new(),
                indices: Vec::new(),
                index_mode: 0,
                alpha_max: 128.0,
                link: Link::Source(257),
                split: None,
                mesh_key: None,
            };
            for (row, (radius, z)) in [(0.634f32, -0.392f32), (0.372, -0.755), (0.196, -0.879)]
                .into_iter()
                .enumerate()
            {
                for col in 0..5 {
                    let angle = (start_deg + 45.0 * col as f32 / 4.0).to_radians();
                    let (s, c) = angle.sin_cos();
                    call.data.extend_from_slice(&[
                        [
                            ((radius * c * 32768.0).round() as i32) as u32,
                            ((radius * s * 32768.0).round() as i32) as u32,
                            ((z * 32768.0).round() as i32) as u32,
                            0,
                        ],
                        [
                            ((1.0 - col as f32 / 4.0) * 4096.0) as u32,
                            row as u32 * 2048,
                            0,
                            0,
                        ],
                        [128; 4],
                        [0; 4],
                    ]);
                }
            }
            call.uniforms[4][1] = 15;
            call
        };
        let p = projection(-1.0, 1.0, -1.0, 1.0, 1.0, 30_000.0);
        for start in [45.0, -90.0] {
            let panels = sky_panorama_panels(&make(start), p).expect("native panel");
            assert_eq!(panels.len(), 2);
            for panel in &panels {
                assert_eq!(panel.uniforms[4][1], 9 * 25);
                assert_eq!(panel.uniforms[4][2..4], [1, 3]);
                assert_eq!(panel.indices.len(), 8 * 24 * 6);
                assert_eq!(panel.link, Link::None);
                assert_eq!(panel.uniforms[0], p[0].map(f32::to_bits));
            }
            assert_ne!(panels[0].data[0], panels[1].data[0]);
        }
    }

    #[test]
    fn wrapped_sky_keeps_native_screen_rays_and_vertex_artwork() {
        use ssx3_render::gpu::{GpuCall, Link, Program, UNIFORM_QWORDS};
        let mut call = GpuCall {
            program: Program::Mat3Mesh,
            pre: Vec::new(),
            tag: 0,
            uniforms: [[0; 4]; UNIFORM_QWORDS],
            data: Vec::new(),
            indices: vec![0, 1, 2, 2, 1, 3],
            index_mode: 0,
            alpha_max: 128.0,
            link: Link::Source(508),
            split: None,
            mesh_key: None,
        };
        let screen = [
            [100.0, 0.0, 0.0, 0.0],
            [0.0, -100.0, 0.0, 0.0],
            [0.0; 4],
            [511.5, 511.5, 0.5, 1.0],
        ];
        for (column, values) in screen.into_iter().enumerate() {
            call.uniforms[column] = values.map(f32::to_bits);
        }
        call.uniforms[18][0] = 1.0f32.to_bits();
        for i in 0..6 {
            let x = i as f32 * 0.3 - 0.75;
            let y = i as f32 * 0.2 - 0.5;
            call.data.extend_from_slice(&[
                [
                    ((x * 32768.0).round() as i32) as u32,
                    ((y * 32768.0).round() as i32) as u32,
                    0,
                    0,
                ],
                [i * 32, i * 64, 0, 0],
                [128, 64, 32, i * 16],
                [0; 4],
            ]);
        }
        call.uniforms[4][1] = 6;
        let p = projection(-1.0, 1.0, -1.0, 1.0, 1.0, 30_000.0);
        let wrapped = sky_wrap_native_panel(&call, p, [0.0, 0.0], (1024, 1024))
            .expect("six-vertex sky strip");
        assert_eq!(wrapped.indices, call.indices);
        assert_eq!(wrapped.uniforms[4][3], 3);
        for i in 0..6 {
            assert_eq!(
                &wrapped.data[i * 4 + 1..i * 4 + 4],
                &call.data[i * 4 + 1..i * 4 + 4]
            );
            let original = call.data[i * 4].map(|v| v as i32 as f32 / 32768.0);
            let hc = transform(screen, [original[0], original[1], original[2], 1.0]);
            let native_ndc = [
                (hc[0] + 0.5) * (2.0 / 1024.0) - 1.0,
                1.0 - (hc[1] + 0.5) * (2.0 / 1024.0),
            ];
            let pos = wrapped.data[i * 4].map(|v| v as i32 as f32 / 32768.0);
            let clip = transform(p, [pos[0], pos[1], pos[2], 1.0]);
            assert!((clip[0] / clip[3] - native_ndc[0]).abs() < 1e-4);
            assert!((clip[1] / clip[3] - native_ndc[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn ridge_peak_strip_keeps_its_native_camera_sector_and_fades_edges() {
        use ssx3_render::gpu::{GpuCall, Link, Program, UNIFORM_QWORDS};
        let mut call = GpuCall {
            program: Program::Mat3Mesh,
            pre: Vec::new(),
            tag: 0,
            uniforms: [[0; 4]; UNIFORM_QWORDS],
            data: Vec::new(),
            indices: vec![0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4, 5],
            index_mode: 0,
            alpha_max: 128.0,
            link: Link::Source(513),
            split: None,
            mesh_key: None,
        };
        let screen = [
            [100.0, 0.0, 0.0, 0.0],
            [0.0, -100.0, 0.0, 0.0],
            [0.0; 4],
            [511.5, 511.5, 0.5, 1.0],
        ];
        for (column, values) in screen.into_iter().enumerate() {
            call.uniforms[column] = values.map(f32::to_bits);
        }
        call.uniforms[18][0] = 1.0f32.to_bits();
        for col in 0..3 {
            let angle = (-45.0 + 22.5 * col as f32).to_radians();
            let (s, co) = angle.sin_cos();
            for (z, v) in [(-0.392f32, 4096), (0.392, 0)] {
                call.data.extend_from_slice(&[
                    [
                        ((0.634 * co * 32768.0).round() as i32) as u32,
                        ((0.634 * s * 32768.0).round() as i32) as u32,
                        ((z * 32768.0).round() as i32) as u32,
                        0,
                    ],
                    [(4096 - 2048 * col), v, 0, 0],
                    [128; 4],
                    [0; 4],
                ]);
            }
        }
        call.uniforms[4][1] = 6;
        // A restored sector captured with an earlier camera must share its
        // edge rays with this frame's live neighbour, retaining its own art.
        let mut cached = call.clone();
        cached.uniforms[3][0] = 580.0f32.to_bits();
        cached.uniforms[18][1] = 0.25f32.to_bits();
        let artwork = cached.data.clone();
        rebase_cached_sky_panel(&mut cached, &call);
        assert_eq!(cached.data, artwork);
        let p = projection(-1.0, 1.0, -1.0, 1.0, 1.0, 30_000.0);
        let panels = sky_native_panorama_panels(
            &call,
            p,
            [0.0, 0.0],
            (1024, 1024),
            true,
            false,
            false,
            true,
        )
        .expect("Ruthless Ridge peak strip");
        let restored = sky_native_panorama_panels(
            &cached,
            p,
            [0.0, 0.0],
            (1024, 1024),
            true,
            false,
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            restored[0].data, panels[0].data,
            "cached/live sector edge rays"
        );
        assert_eq!(panels.len(), 1);
        let panel = &panels[0];
        assert_eq!(panel.uniforms[4][1], 17 * 9);
        assert_eq!(panel.uniforms[4][2..4], [1, 3]);
        assert_eq!(panel.indices.len(), 16 * 8 * 6);
        assert_eq!(panel.link, Link::None);
        assert_eq!(panel.data[1][0..2], [4096, 0]);
        assert_eq!(panel.data[2][3], 0);
        assert_eq!(panel.data[4 * 4 + 2][3], 128);
        let bottom_left = 16 * 9 * 4;
        assert_eq!(panel.data[bottom_left + 1][0..2], [4096, 4096]);
        let snow_upper = sky_native_panorama_panels(
            &call,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            snow_upper[0].data[4 * 4 + 2][3],
            0,
            "open top fades into sky"
        );
        assert_eq!(snow_upper[0].data[(2 * 9 + 4) * 4 + 2][3], 128);
        for (texture, first, last) in [(286, 0, 128), (284, 128, 128), (283, 128, 0)] {
            let sector =
                snow_sky_panorama_panels(&call, p, [0.0, 0.0], (1024, 1024), texture).unwrap();
            // This fixture is rotated relative to Snow Jam's other meshes;
            // artwork indices still identify the two open side boundaries.
            assert_eq!(sector[0].data[(8 * 9) * 4 + 2][3], first);
            assert_eq!(sector[0].data[(8 * 9 + 8) * 4 + 2][3], last);
        }
        // Even sectors never submitted by the forward camera have valid
        // native artwork bounds on the first tracked frame.
        for id in [289, 288, 287, 282, 286, 285, 284, 283] {
            let source = snow_sky_sector(&call, 285, id).unwrap();
            let sector = sky_native_panorama_panels(
                &source,
                p,
                [0.0, 0.0],
                (1024, 1024),
                false,
                false,
                false,
                false,
            )
            .expect("cold-cache Snow Jam sector");
            assert_eq!(sector[0].data.len(), 17 * 9 * 4);
        }
        // Gravitude's neighbouring upper strips meet at -45° and 0°.
        // Keep their artwork opaque there; an unpaired outer edge must
        // still fade into the blue dome instead of exposing a hard wedge.
        let joined_upper = sky_native_panorama_panels(
            &call,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            true,
            false,
            true,
        )
        .expect("joined upper sector");
        assert_eq!(joined_upper[0].data[2][3], 128);
        assert_eq!(joined_upper[0].data[8 * 4 + 2][3], 128);
        let crossfade_upper =
            sky_native_panorama_panels(&call, p, [0.0, 0.0], (1024, 1024), false, true, true, true)
                .expect("crossfaded upper sector");
        assert_eq!(crossfade_upper[0].data[14 * 9 * 4 + 2][3], 128);
        assert_eq!(crossfade_upper[0].data[16 * 9 * 4 + 2][3], 0);
        let mut outer = call.clone();
        for vertex in outer.data.chunks_exact_mut(4) {
            let x = vertex[0][0] as i32 as f32 / 32768.0;
            let y = vertex[0][1] as i32 as f32 / 32768.0;
            let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
            vertex[0][0] = (((c * x + s * y) * 32768.0).round() as i32) as u32;
            vertex[0][1] = (((-s * x + c * y) * 32768.0).round() as i32) as u32;
        }
        let outer = sky_native_panorama_panels(
            &outer,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            true,
            false,
            true,
        )
        .expect("outer upper sector");
        assert_eq!(outer[0].data[2][3], 0);
        assert_eq!(outer[0].data[8 * 4 + 2][3], 128);
        for (source, wrapped) in [(1, 0), (0, bottom_left)] {
            let original = call.data[source * 4].map(|v| v as i32 as f32 / 32768.0);
            let hc = transform(screen, [original[0], original[1], original[2], 1.0]);
            let native_ndc = [
                (hc[0] + 0.5) * (2.0 / 1024.0) - 1.0,
                1.0 - (hc[1] + 0.5) * (2.0 / 1024.0),
            ];
            let pos = panel.data[wrapped].map(|v| v as i32 as f32 / 32768.0);
            let clip = transform(p, [pos[0], pos[1], pos[2], 1.0]);
            assert!((clip[0] / clip[3] - native_ndc[0]).abs() < 1e-4);
            assert!((clip[1] / clip[3] - native_ndc[1]).abs() < 1e-4);
        }
        // Gravitude's lower sector meets its upper artwork at this edge;
        // fading it leaves a visible horizontal stripe in the forward view.
        let mut lower = call.clone();
        lower.data.clear();
        for (row, (radius, z)) in [(0.634f32, -0.392f32), (0.372, -0.755), (0.196, -0.879)]
            .into_iter()
            .enumerate()
        {
            for col in 0..5 {
                let angle = (-45.0 + 11.25 * col as f32).to_radians();
                let (s, co) = angle.sin_cos();
                lower.data.extend_from_slice(&[
                    [
                        ((radius * co * 32768.0).round() as i32) as u32,
                        ((radius * s * 32768.0).round() as i32) as u32,
                        ((z * 32768.0).round() as i32) as u32,
                        0,
                    ],
                    [4096 - 1024 * col, row as u32 * 2048, 0, 0],
                    [128; 4],
                    [0; 4],
                ]);
            }
        }
        lower.uniforms[4][1] = 15;
        let faded = sky_native_panorama_panels(
            &lower,
            p,
            [0.0, 0.0],
            (1024, 1024),
            true,
            false,
            false,
            true,
        )
        .expect("lower sector");
        let joined = sky_native_panorama_panels(
            &lower,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            false,
            false,
            true,
        )
        .expect("joined lower sector");
        assert_eq!(faded[0].data[4 * 4 + 2][3], 0);
        assert_eq!(joined[0].data[4 * 4 + 2][3], 128);
        // Snow Jam's adjacent sectors should meet without a blue alpha seam.
        let seamless = sky_native_panorama_panels(
            &lower,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            false,
            false,
            false,
        )
        .expect("seamless lower sector");
        assert_eq!(seamless[0].data[2][3], 128);
        assert_eq!(seamless[0].data[8 * 4 + 2][3], 128);
        let rotated = |degrees: f32| {
            let mut sector = lower.clone();
            let (sn, co) = degrees.to_radians().sin_cos();
            for vertex in sector.data.chunks_exact_mut(4) {
                let x = vertex[0][0] as i32 as f32 / 32768.0;
                let y = vertex[0][1] as i32 as f32 / 32768.0;
                vertex[0][0] = (((co * x - sn * y) * 32768.0).round() as i32) as u32;
                vertex[0][1] = (((sn * x + co * y) * 32768.0).round() as i32) as u32;
            }
            sector
        };
        let first_snow = sky_native_panorama_panels(
            &rotated(135.0),
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            false,
            false,
            false,
        )
        .expect("first Snow Jam sector");
        assert_eq!(first_snow[0].data[2][3], 0);
        assert_eq!(first_snow[0].data[4 * 4 + 2][3], 128);
        assert_eq!(first_snow[0].data[8 * 4 + 2][3], 128);
        let last_snow = sky_native_panorama_panels(
            &rotated(-90.0),
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            false,
            false,
            false,
        )
        .expect("last Snow Jam sector");
        assert_eq!(last_snow[0].data[2][3], 128);
        assert_eq!(last_snow[0].data[4 * 4 + 2][3], 128);
        assert_eq!(last_snow[0].data[8 * 4 + 2][3], 0);
        let crossfade_lower = sky_native_panorama_panels(
            &lower,
            p,
            [0.0, 0.0],
            (1024, 1024),
            false,
            true,
            true,
            true,
        )
        .expect("crossfaded lower sector");
        assert_eq!(crossfade_lower[0].uniforms[4][1], 19 * 9);
        assert_eq!(crossfade_lower[0].data[1][1], 0);
        assert!(crossfade_lower[0].data[2 * 9 * 4 + 1][1] > 0);
        assert_eq!(crossfade_lower[0].data[18 * 9 * 4 + 1][1], 4096);
    }
}
