//! Host-only refinement of the native Bézier terrain triangles. The original
//! VU grid and all guest state stay intact; the two texture passes reuse the
//! same refined vertices and interpolate their own UVs.

use std::collections::{HashMap, HashSet};

use ssx3_render::bezier::gpu::{self as bz, unpack_param};
use ssx3_render::gpu::{GpuCall, Link};

type Q = [u32; 4];

#[derive(Clone, Copy)]
struct Blend {
    vertices: [usize; 3],
    weights: [f32; 3],
}

/// The latest `Link::Source` terrain mesh, also used by later ST rewrites.
pub struct Mesh {
    source_vertices: usize,
    endings: HashMap<u32, Vec<u32>>,
    appended: Vec<Blend>,
}

#[derive(Default)]
pub struct Stats {
    pub patches: usize,
    pub triangles: usize,
    pub rejected: usize,
    pub candidates: usize,
    pub measured: usize,
    pub max_error: f32,
    pub visible_sources: usize,
}

#[derive(Clone, Copy)]
pub struct EyeProject {
    pub reproject: [[f32; 4]; 4],
    pub game_size: (u32, u32),
    pub offset: [f32; 2],
}

fn f(q: Q) -> [f32; 4] {
    q.map(f32::from_bits)
}

fn bits(q: [f32; 4]) -> Q {
    q.map(f32::to_bits)
}

fn eval(c: &[[u32; 4]; 16], [u, v]: [f32; 2]) -> [f32; 4] {
    let up = [u * u * u, u * u, u, 1.0];
    let vp = [v * v * v, v * v, v, 1.0];
    let mut out = [0.0; 4];
    for j in 0..4 {
        for i in 0..4 {
            let q = f(c[4 * j + i]);
            for lane in 0..4 {
                out[lane] += q[lane] * up[i] * vp[j];
            }
        }
    }
    out
}

fn point(data: &[Q], v: usize) -> [f32; 4] {
    f(data[3 * v])
}

fn params(data: &[Q], v: usize) -> Option<[f32; 2]> {
    unpack_param(data[3 * v + 2][3])
}

fn screen(call: &GpuCall, p: [f32; 4], eye: EyeProject) -> Option<[f32; 2]> {
    let mut h = [0.0; 4];
    for (col, &value) in p.iter().enumerate() {
        let m = f(call.uniforms[col]);
        for lane in 0..4 {
            h[lane] += m[lane] * value;
        }
    }
    if !h.iter().all(|v| v.is_finite()) {
        return None;
    }
    let gs = f(call.uniforms[4]);
    let go = f(call.uniforms[5]);
    let w = h[3];
    let (sx, sy) = (eye.game_size.0 as f32, eye.game_size.1 as f32);
    let clip = [
        (gs[0] * h[0] + (go[0] - eye.offset[0] + 0.5) * w) * 2.0 / sx - w,
        w - (gs[1] * h[1] + (go[1] - eye.offset[1] + 0.5) * w) * 2.0 / sy,
        h[2],
        w,
    ];
    let h = crate::vr::transform(eye.reproject, clip);
    if h[3] <= 0.1
        || h[0].abs() > 1.25 * h[3]
        || h[1].abs() > 1.25 * h[3]
        || !h.iter().all(|v| v.is_finite())
    {
        return None;
    }
    Some([512.0 * (h[0] / h[3] + 1.0), 512.0 * (h[1] / h[3] + 1.0)])
}

fn blend3(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}

fn shared_border(a: [f32; 2], b: [f32; 2]) -> bool {
    let eq = |x: f32, y: f32| (x - y).abs() < 1e-4;
    (eq(a[0], b[0]) && (eq(a[0], 0.0) || eq(a[0], 1.0)))
        || (eq(a[1], b[1]) && (eq(a[1], 0.0) || eq(a[1], 1.0)))
}

/// Error in a nominal 1024-pixel eye between a coarse triangle edge and the
/// exact cubic midpoint. Reject vertices that do not map to this patch.
fn edge_error(
    call: &GpuCall,
    data: &[Q],
    c: &[[u32; 4]; 16],
    a: usize,
    b: usize,
    eyes: &[EyeProject; 2],
) -> Option<f32> {
    let (pa, pb) = (params(data, a)?, params(data, b)?);
    let (va, vb) = (point(data, a), point(data, b));
    let linear = blend3(va, vb, 0.5);
    let mut midpoint = if shared_border(pa, pb) {
        linear
    } else {
        eval(c, [(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5])
    };
    // The VU's `sqc2` masks out the cubic's W lane. The grid's W (usually
    // 1) is retained even though the coefficient's unused W polynomial is
    // not constant.
    midpoint[3] = linear[3];
    eyes.iter()
        .filter_map(|&eye| {
            let x = screen(call, linear, eye)?;
            let y = screen(call, midpoint, eye)?;
            let d = (x[0] - y[0]).hypot(x[1] - y[1]);
            d.is_finite().then_some(d)
        })
        .reduce(f32::max)
}

fn valid_vertex(data: &[Q], c: &[[u32; 4]; 16], v: usize) -> bool {
    let Some(uv) = params(data, v) else {
        return false;
    };
    let original = point(data, v);
    let exact = eval(c, uv);
    // A coarse type-2 zipper can move its edge vertices. It must keep
    // its original shape rather than be replaced by a mismatched cubic.
    let delta = (0..3)
        .map(|i| (original[i] - exact[i]).abs())
        .fold(0.0, f32::max);
    delta.is_finite() && delta < 1.0
}

fn edge_key(data: &[Q], a: usize, b: usize) -> (u32, u32) {
    let x = data[3 * a + 2][3];
    let y = data[3 * b + 2][3];
    (x.min(y), x.max(y))
}

fn refined_vertex(
    data: &[Q],
    c: &[[u32; 4]; 16],
    blend: Blend,
    curved: &HashSet<(u32, u32)>,
) -> [Q; 3] {
    let mut p = [0.0; 4];
    let mut uv = [0.0; 4];
    let mut n = [0.0; 4];
    let mut param = [0.0; 2];
    for k in 0..3 {
        let v = blend.vertices[k];
        let w = blend.weights[k];
        let vp = point(data, v);
        let vu = f(data[3 * v + 1]);
        let vn = f(data[3 * v + 2]);
        let pq = params(data, v).unwrap();
        for lane in 0..4 {
            p[lane] += w * vp[lane];
            uv[lane] += w * vu[lane];
            n[lane] += w * vn[lane];
        }
        for lane in 0..2 {
            param[lane] += w * pq[lane];
        }
    }
    let edge = blend
        .vertices
        .iter()
        .enumerate()
        .filter(|(k, _)| blend.weights[*k] > 0.0)
        .map(|(_, &v)| v)
        .collect::<Vec<_>>();
    if edge.len() == 2 && curved.contains(&edge_key(data, edge[0], edge[1])) {
        let exact = eval(c, param);
        p[..3].copy_from_slice(&exact[..3]);
    }
    let len = n[..3].iter().map(|x| x * x).sum::<f32>().sqrt();
    if len > 0.0 && len.is_finite() {
        for x in &mut n[..3] {
            *x /= len;
        }
    }
    // The parameter slot is not read by the present shader.
    n[3] = 0.0;
    [bits(p), bits(uv), bits(n)]
}

impl Mesh {
    fn build(
        calls: &[GpuCall],
        c: &[[u32; 4]; 16],
        eyes: &[EyeProject; 2],
        remaining: &mut usize,
        stats: &mut Stats,
    ) -> Option<(Self, Vec<Q>)> {
        let source = calls.first()?;
        let nv = source.uniforms[7][2] as usize;
        if source.program != ssx3_render::gpu::Program::Bezier
            || source.data.len() != 3 * nv
            || nv == 0
            || nv > 4096
        {
            stats.rejected += 1;
            return None;
        }
        if !(0..nv).any(|v| {
            eyes.iter()
                .any(|&eye| screen(source, point(&source.data, v), eye).is_some())
        }) {
            return None;
        }
        stats.visible_sources += 1;
        let valid: Vec<_> = (0..nv).map(|v| valid_vertex(&source.data, c, v)).collect();
        let mut selected = Vec::new();
        let mut endings = HashMap::new();
        let mut seen = HashSet::new();
        let mut patch_triangles = 0;
        for call in calls
            .iter()
            .filter(|call| call.uniforms[7][0] == bz::STRIPS)
        {
            for &i in &call.indices {
                if i < 2 || i as usize >= nv || !seen.insert(i) {
                    continue;
                }
                let verts = [i as usize - 2, i as usize - 1, i as usize];
                stats.candidates += 1;
                let error = if verts.iter().all(|&v| valid[v]) {
                    let mut measured = false;
                    let mut error = 0.0f32;
                    for (a, b) in [
                        (verts[0], verts[1]),
                        (verts[1], verts[2]),
                        (verts[2], verts[0]),
                    ] {
                        if let Some(e) = edge_error(source, &source.data, c, a, b, eyes) {
                            measured = true;
                            error = error.max(e);
                        }
                    }
                    if measured {
                        stats.measured += 1;
                    }
                    error
                } else {
                    0.0
                };
                stats.max_error = stats.max_error.max(error);
                if error < 0.7 || *remaining == 0 || patch_triangles >= 32 {
                    endings.insert(i, vec![i]);
                    continue;
                }
                *remaining -= 1;
                patch_triangles += 1;
                stats.triangles += 1;
                selected.push((i, verts));
            }
        }
        if selected.is_empty() {
            return None;
        }
        // The strips duplicate grid vertices. Canonical parameter pairs
        // identify a shared edge even across two distinct strip records.
        // Curved midpoints are allowed only when both adjacent triangles
        // refine it; an edge bordering a coarse triangle or another patch
        // stays exactly on the original straight segment. This keeps the
        // adaptive mesh watertight at LOD transitions.
        let mut edge_counts: HashMap<(u32, u32), u8> = HashMap::new();
        for (_, [a, b, c]) in &selected {
            for (x, y) in [(*a, *b), (*b, *c), (*c, *a)] {
                *edge_counts.entry(edge_key(&source.data, x, y)).or_default() += 1;
            }
        }
        let curved: HashSet<_> = edge_counts
            .into_iter()
            .filter_map(|(key, n)| (n == 2).then_some(key))
            .collect();
        let mut data = source.data.clone();
        let mut appended = Vec::new();
        let a = [1.0, 0.0, 0.0];
        let b = [0.0, 1.0, 0.0];
        let cc = [0.0, 0.0, 1.0];
        let ab = [0.5, 0.5, 0.0];
        let bc = [0.0, 0.5, 0.5];
        let ca = [0.5, 0.0, 0.5];
        let triangles = [[a, ab, ca], [ab, b, bc], [ca, bc, cc], [ab, bc, ca]];
        for (i, verts) in selected {
            let mut ids = Vec::with_capacity(4);
            for tri in triangles {
                for weights in tri {
                    let blend = Blend {
                        vertices: verts,
                        weights,
                    };
                    data.extend_from_slice(&refined_vertex(&source.data, c, blend, &curved));
                    appended.push(blend);
                }
                ids.push((data.len() / 3 - 1) as u32);
            }
            endings.insert(i, ids);
        }
        stats.patches += 1;
        Some((
            Self {
                source_vertices: nv,
                endings,
                appended,
            },
            data,
        ))
    }

    fn apply(&self, call: &mut GpuCall) {
        if call.uniforms[7][1] != 0 {
            if call.data.len() != self.source_vertices {
                // Keep this pass on the original prefix of the source data.
                return;
            }
            let original = call.data.clone();
            for blend in &self.appended {
                let mut uv = [0.0; 4];
                for k in 0..3 {
                    let p = f(original[blend.vertices[k]]);
                    for lane in 0..4 {
                        uv[lane] += blend.weights[k] * p[lane];
                    }
                }
                call.data.push(bits(uv));
            }
        }
        call.uniforms[7][2] = (self.source_vertices + self.appended.len()) as u32;
        // Keep the fan call even in VR: it owns and uploads the Source data
        // before the following strip's Link::Reuse. The VR shader kills its
        // clipped duplicate triangles at draw time.
        if call.uniforms[7][0] == bz::STRIPS {
            let mut indices = Vec::with_capacity(call.indices.len() * 2);
            for &i in &call.indices {
                if let Some(ids) = self.endings.get(&i) {
                    indices.extend_from_slice(ids);
                } else {
                    indices.push(i);
                }
            }
            call.indices = indices;
        }
    }
}

/// Transform the output of one native VU call. `cache` follows the VU/GPU
/// `Link::Source` lifetime, including projector ST rewrites in later calls.
pub fn refine_calls(
    calls: &mut [GpuCall],
    coeffs: Option<&[[u32; 4]; 16]>,
    eyes: Option<&[EyeProject; 2]>,
    cache: &mut Option<Mesh>,
    remaining: &mut usize,
    stats: &mut Stats,
) {
    let mut start = 0;
    while start < calls.len() {
        let end = (start + 1..calls.len())
            .find(|&i| matches!(calls[i].link, Link::Source(_)))
            .unwrap_or(calls.len());
        if matches!(calls[start].link, Link::Source(bz::KEY)) {
            let built = coeffs
                .zip(eyes)
                .and_then(|(c, eyes)| Mesh::build(&calls[start..end], c, eyes, remaining, stats));
            if let Some((mesh, data)) = built {
                let mut data = Some(data);
                for (j, call) in calls[start..end].iter_mut().enumerate() {
                    if j == 0 {
                        call.data = data.take().unwrap();
                    }
                    mesh.apply(call);
                }
                *cache = Some(mesh);
            } else {
                *cache = None;
            }
        } else if let Some(mesh) = cache.as_ref() {
            for call in &mut calls[start..end] {
                if matches!(call.link, Link::Reuse(bz::KEY))
                    && call.program == ssx3_render::gpu::Program::Bezier
                {
                    mesh.apply(call);
                }
            }
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx3_render::gpu::{tri_mode, Program};

    fn source_vertex(x: f32, u: u16, v: u16) -> [Q; 3] {
        [
            bits([x, 0.0, 0.0, 1.0]),
            [0; 4],
            [
                0,
                0,
                1f32.to_bits(),
                0x8000_0000 | ((u as u32) << 15) | v as u32,
            ],
        ]
    }

    #[test]
    fn midpoint_curves_only_when_the_adjacent_triangles_share_it() {
        let mut data = Vec::new();
        data.extend_from_slice(&source_vertex(0.0, 0, 0));
        data.extend_from_slice(&source_vertex(1.0, 32767, 0));
        data.extend_from_slice(&source_vertex(0.0, 0, 32767));
        let mut coeffs = [[0; 4]; 16];
        // x(u,v) = u²; the original straight edge has x=0.5 at u=0.5.
        coeffs[13][0] = 1f32.to_bits();
        let blend = Blend {
            vertices: [0, 1, 2],
            weights: [0.5, 0.5, 0.0],
        };
        let linear = refined_vertex(&data, &coeffs, blend, &HashSet::new());
        let curved = refined_vertex(
            &data,
            &coeffs,
            blend,
            &HashSet::from([edge_key(&data, 0, 1)]),
        );
        assert_eq!(f(linear[0])[0], 0.5);
        assert_eq!(f(curved[0])[0], 0.25);
        assert_eq!(f(linear[0])[3], 1.0);
        assert_eq!(f(curved[0])[3], 1.0);
    }

    #[test]
    fn second_pass_reuses_the_refined_topology_and_interpolates_its_uvs() {
        let blend = Blend {
            vertices: [0, 1, 2],
            weights: [0.5, 0.5, 0.0],
        };
        let mesh = Mesh {
            source_vertices: 3,
            endings: HashMap::from([(2, vec![5])]),
            appended: vec![blend, blend, blend],
        };
        let mut uniforms = [[0; 4]; 32];
        uniforms[7] = [bz::STRIPS, 1, 3, 1];
        let mut call = GpuCall {
            program: Program::Bezier,
            pre: Vec::new(),
            tag: 0,
            uniforms,
            data: vec![
                bits([0.0, 0.0, 0.0, 0.0]),
                bits([1.0, 0.0, 0.0, 0.0]),
                bits([0.0, 1.0, 0.0, 0.0]),
            ],
            indices: vec![2],
            index_mode: tri_mode(3, 2),
            alpha_max: 0.0,
            link: Link::Reuse(bz::KEY),
            split: None,
            mesh_key: None,
        };
        mesh.apply(&mut call);
        assert_eq!(call.uniforms[7][2], 6);
        assert_eq!(call.indices, [5]);
        assert_eq!(call.data.len(), 6);
        assert_eq!(f(call.data[3])[0], 0.5);
    }
}
