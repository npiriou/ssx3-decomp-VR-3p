//! Source-space smoothing and albedo projection of unlit rock meshes for VR.
//! A Gravitude rock can span several native calls. Weld their original
//! fixed-point positions before moving any point, so shared edges stay
//! watertight. One-call groups provide negligible smoothing gain here;
//! their albedo can still be projected without waiting for another frame.

use std::collections::{HashMap, HashSet};

use ssx3_render::gpu::{GpuCall, Link, Program};

type Edge = (usize, usize);

type Q = [u32; 4];

#[derive(Clone)]
struct Ready {
    source: Vec<Q>,
    indices: Vec<u32>,
    refined: Vec<Q>,
    refined_indices: Vec<u32>,
}

/// Source meshes are static even when their instance matrix changes. The
/// first frame gathers calls in their native order; later frames apply the
/// same watertight topology as each VU result arrives, without waiting for
/// the rest of that frame's VU work.
#[derive(Clone, Default)]
pub struct Cache {
    strength_bits: u32,
    tile_bits: u32,
    ready: HashMap<u64, Vec<Ready>>,
    pending: HashMap<(usize, [Q; 4]), Vec<(u64, GpuCall, bool)>>,
}

fn signature(call: &GpuCall) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in call.data.iter().flatten().chain(call.indices.iter()) {
        hash = (hash ^ *value as u64).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

impl Cache {
    pub fn begin_frame(&mut self, strength: f32, tile: f32) {
        if self.strength_bits != strength.to_bits() || self.tile_bits != tile.to_bits() {
            self.ready.clear();
            self.strength_bits = strength.to_bits();
            self.tile_bits = tile.to_bits();
        }
        self.pending.clear();
    }

    pub fn transform(&mut self, draw: usize, call: &mut GpuCall) -> bool {
        if call.program != Program::Mat3Mesh
            || !matches!(call.link, Link::Source(_))
            || call.uniforms[4][0] != 0
            || call.uniforms[4][2] != 1
            || call.index_mode != 0
        {
            return false;
        }
        let key = signature(call);
        let screen: [Q; 4] = call.uniforms[..4].try_into().unwrap();
        let mesh = self.ready.get(&key).and_then(|variants| {
            variants
                .iter()
                .find(|mesh| mesh.source == call.data && mesh.indices == call.indices)
        });
        self.pending
            .entry((draw, screen))
            .or_default()
            .push((key, call.clone(), mesh.is_some()));
        let Some(mesh) = mesh else {
            let tile = f32::from_bits(self.tile_bits);
            return tile > 0.0 && tile_call(call, tile);
        };
        call.data = mesh.refined.clone();
        call.indices = mesh.refined_indices.clone();
        call.uniforms[4][1] = (call.data.len() / 4) as u32;
        call.mesh_key = None;
        true
    }

    pub fn finish_frame(&mut self) -> Vec<SmoothStats> {
        let mut stats = Vec::new();
        for group in std::mem::take(&mut self.pending).into_values() {
            if group.iter().all(|(_, _, hit)| *hit) {
                continue;
            }
            let mut calls: Vec<_> = group.iter().map(|(_, call, _)| call.clone()).collect();
            let strength = f32::from_bits(self.strength_bits);
            let tile = f32::from_bits(self.tile_bits);
            let smoothed = if group.len() >= 2 && strength > 0.0 {
                smooth_group(&mut calls, strength)
            } else {
                None
            };
            let tiled = if tile > 0.0 {
                let mut projected = calls.clone();
                if projected.iter_mut().all(|call| tile_call(call, tile)) {
                    calls = projected;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if smoothed.is_none() && !tiled {
                continue;
            }
            if let Some(stat) = smoothed {
                stats.push(stat);
            }
            for ((key, old, _), new) in group.into_iter().zip(calls) {
                let variants = self.ready.entry(key).or_default();
                if !variants
                    .iter()
                    .any(|mesh| mesh.source == old.data && mesh.indices == old.indices)
                {
                    variants.push(Ready {
                        source: old.data,
                        indices: old.indices,
                        refined: new.data,
                        refined_indices: new.indices,
                    });
                }
            }
        }
        if self.ready.len() > 1024 {
            self.ready.clear();
        }
        stats
    }
}

fn edge(a: usize, b: usize) -> Edge {
    (a.min(b), a.max(b))
}

fn position(v: &[u32; 4]) -> [f32; 3] {
    std::array::from_fn(|i| v[i] as i32 as f32 / 32768.0)
}

fn fixed(p: [f32; 3]) -> [u32; 3] {
    p.map(|x| ((x * 32768.0).round() as i32) as u32)
}

/// Retile the rock's native albedo in source space. Each triangle chooses
/// its dominant local plane, so a steep face does not stretch one texture
/// row across the whole cliff. Duplicate records at projection seams while
/// keeping the original positions, colours and GIF draw order.
fn tile_call(call: &mut GpuCall, tile: f32) -> bool {
    let n = call.uniforms[4][1] as usize;
    if n == 0
        || call.data.len() != 4 * n
        || call.indices.is_empty()
        || call.indices.len() % 3 != 0
        || call.indices.iter().any(|&i| i as usize >= n)
    {
        return false;
    }
    let mut data = Vec::with_capacity(call.indices.len() * 4);
    let mut indices = Vec::with_capacity(call.indices.len());
    for tri in call.indices.chunks_exact(3) {
        let p = [tri[0], tri[1], tri[2]].map(|i| position(&call.data[4 * i as usize]));
        let a = std::array::from_fn::<_, 3, _>(|lane| p[1][lane] - p[0][lane]);
        let b = std::array::from_fn::<_, 3, _>(|lane| p[2][lane] - p[0][lane]);
        let normal = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let major = (0..3)
            .max_by(|&x, &y| normal[x].abs().total_cmp(&normal[y].abs()))
            .unwrap();
        let (u, v) = match major {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        for (&i, point) in tri.iter().zip(p) {
            let src = &call.data[4 * i as usize..4 * i as usize + 4];
            let mut record: [[u32; 4]; 4] = src.try_into().unwrap();
            record[1][0] = ((point[u] * tile * 4096.0).round() as i32) as u32;
            record[1][1] = ((point[v] * tile * 4096.0).round() as i32) as u32;
            indices.push((data.len() / 4) as u32);
            data.extend_from_slice(&record);
        }
    }
    call.uniforms[4][1] = (data.len() / 4) as u32;
    call.data = data;
    call.indices = indices;
    call.mesh_key = None;
    true
}

fn midpoint_record(a: &[[u32; 4]], b: &[[u32; 4]], p: [u32; 3]) -> [[u32; 4]; 4] {
    let mut out = [a[0], a[1], a[2], [0; 4]];
    out[0][..3].copy_from_slice(&p);
    for (q, count) in [(1, 2), (2, 4)] {
        for lane in 0..count {
            let sum = a[q][lane] as i32 as i64 + b[q][lane] as i32 as i64;
            out[q][lane] = (sum / 2) as i32 as u32;
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct SmoothStats {
    pub source_vertices: usize,
    pub source_triangles: usize,
    pub triangles: usize,
}

/// One Loop subdivision step, blended with the original geometry by
/// `strength`. Interior edges use their two adjacent faces. An open boundary
/// uses its ordered neighbours but retains its original vertices; ambiguous
/// or non-manifold groups are left untouched.
pub fn smooth_group(calls: &mut [GpuCall], strength: f32) -> Option<SmoothStats> {
    if !(0.0..=1.0).contains(&strength) || strength == 0.0 || calls.is_empty() {
        return None;
    }
    let mut ids = Vec::with_capacity(calls.len());
    let mut lookup = HashMap::<[u32; 3], usize>::new();
    let mut points = Vec::<[f32; 3]>::new();
    for call in calls.iter() {
        let n = call.uniforms[4][1] as usize;
        if call.program != Program::Mat3Mesh
            || !matches!(call.link, Link::Source(_))
            || call.uniforms[4][0] != 0
            || call.uniforms[4][2] != 1
            || n == 0
            || n > 512
            || call.data.len() != 4 * n
            || call.indices.len() % 3 != 0
            || call.index_mode != 0
        {
            return None;
        }
        let mut call_ids = Vec::with_capacity(n);
        for v in call.data.chunks_exact(4) {
            let raw = [v[0][0], v[0][1], v[0][2]];
            let id = *lookup.entry(raw).or_insert_with(|| {
                points.push(position(&v[0]));
                points.len() - 1
            });
            call_ids.push(id);
        }
        ids.push(call_ids);
    }
    if points.len() < 4 || points.len() > 4096 {
        return None;
    }
    let mut opposites = HashMap::<Edge, Vec<usize>>::new();
    let mut source_triangles = 0;
    for (call, call_ids) in calls.iter().zip(&ids) {
        for tri in call.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if [a, b, c].iter().any(|&v| v >= call_ids.len()) {
                return None;
            }
            let [a, b, c] = [call_ids[a], call_ids[b], call_ids[c]];
            if a == b || b == c || c == a {
                continue;
            }
            source_triangles += 1;
            opposites.entry(edge(a, b)).or_default().push(c);
            opposites.entry(edge(b, c)).or_default().push(a);
            opposites.entry(edge(c, a)).or_default().push(b);
        }
    }
    if source_triangles < 2 || opposites.values().any(|v| v.is_empty() || v.len() > 2) {
        return None;
    }
    let mut neighbors = vec![HashSet::<usize>::new(); points.len()];
    let mut boundary = vec![Vec::<usize>::new(); points.len()];
    for (&(a, b), other) in &opposites {
        neighbors[a].insert(b);
        neighbors[b].insert(a);
        if other.len() == 1 {
            boundary[a].push(b);
            boundary[b].push(a);
        }
    }
    if neighbors
        .iter()
        .enumerate()
        .any(|(id, ns)| ns.len() < 2 || (!boundary[id].is_empty() && boundary[id].len() != 2))
    {
        return None;
    }
    let old_positions: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(id, &p)| {
            let mut ns: Vec<_> = neighbors[id].iter().copied().collect();
            ns.sort_unstable();
            if !boundary[id].is_empty() {
                return fixed(p);
            }
            let n = ns.len();
            let angle = std::f32::consts::TAU / n as f32;
            let beta = (5.0 / 8.0 - (3.0 / 8.0 + angle.cos() / 4.0).powi(2)) / n as f32;
            let mut out = p;
            for lane in 0..3 {
                let sum: f32 = ns.iter().map(|&other| points[other][lane]).sum();
                out[lane] += strength * beta * (sum - n as f32 * p[lane]);
            }
            fixed(out)
        })
        .collect();
    let edge_positions: HashMap<_, _> = opposites
        .iter()
        .map(|(&(a, b), other)| {
            let pa = points[a];
            let pb = points[b];
            let out = std::array::from_fn(|lane| {
                let linear = (pa[lane] + pb[lane]) * 0.5;
                if other.len() == 2 {
                    let pc = points[other[0]];
                    let pd = points[other[1]];
                    linear + strength * (pc[lane] + pd[lane] - pa[lane] - pb[lane]) * 0.125
                } else {
                    let prev = points[*boundary[a].iter().find(|&&v| v != b).unwrap()];
                    let next = points[*boundary[b].iter().find(|&&v| v != a).unwrap()];
                    linear + strength * (pa[lane] + pb[lane] - prev[lane] - next[lane]) / 16.0
                }
            });
            ((a, b), fixed(out))
        })
        .collect();
    for (call, call_ids) in calls.iter_mut().zip(ids) {
        let original = std::mem::take(&mut call.data);
        let mut data = original.clone();
        for (i, &id) in call_ids.iter().enumerate() {
            data[4 * i][..3].copy_from_slice(&old_positions[id]);
        }
        let mut midpoints = HashMap::<Edge, u32>::new();
        let mut indices = Vec::with_capacity(call.indices.len() * 4);
        for tri in call.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]];
            let [ia, ib, ic] = [
                call_ids[a as usize],
                call_ids[b as usize],
                call_ids[c as usize],
            ];
            if ia == ib || ib == ic || ic == ia {
                continue;
            }
            let mut midpoint = |x: u32, y: u32, ix: usize, iy: usize| {
                let key = edge(ix, iy);
                *midpoints.entry(key).or_insert_with(|| {
                    let first = &original[4 * x as usize..4 * x as usize + 4];
                    let second = &original[4 * y as usize..4 * y as usize + 4];
                    let vertex = midpoint_record(first, second, edge_positions[&key]);
                    let id = (data.len() / 4) as u32;
                    data.extend_from_slice(&vertex);
                    id
                })
            };
            let ab = midpoint(a, b, ia, ib);
            let bc = midpoint(b, c, ib, ic);
            let ca = midpoint(c, a, ic, ia);
            indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
        }
        call.uniforms[4][1] = (data.len() / 4) as u32;
        call.data = data;
        call.indices = indices;
        call.mesh_key = None;
    }
    Some(SmoothStats {
        source_vertices: points.len(),
        source_triangles,
        triangles: source_triangles * 4,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(faces: &[[u32; 3]]) -> GpuCall {
        let positions = [[0, 0, 32768], [32768, 0, 0], [0, 32768, 0], [0, 0, 0]];
        let mut data = Vec::new();
        for p in positions {
            data.extend_from_slice(&[[p[0], p[1], p[2], 0], [0; 4], [128; 4], [0; 4]]);
        }
        let mut uniforms = [[0; 4]; 32];
        uniforms[4] = [0, 4, 1, 0];
        GpuCall {
            program: Program::Mat3Mesh,
            pre: Vec::new(),
            tag: 0,
            uniforms,
            data,
            indices: faces.iter().flatten().copied().collect(),
            index_mode: 0,
            alpha_max: 128.0,
            link: Link::Source(1),
            split: None,
            mesh_key: None,
        }
    }

    #[test]
    fn shared_edges_get_identical_positions_across_calls() {
        let mut calls = [call(&[[0, 1, 2], [0, 3, 1]]), call(&[[0, 2, 3], [1, 3, 2]])];
        let stats = smooth_group(&mut calls, 0.7).unwrap();
        assert_eq!(
            (
                stats.source_vertices,
                stats.source_triangles,
                stats.triangles
            ),
            (4, 4, 16)
        );
        assert_eq!(calls[0].data.len(), calls[1].data.len());
        assert_eq!(calls[0].indices.len(), 24);
        for i in 0..4 {
            assert_eq!(calls[0].data[4 * i][..3], calls[1].data[4 * i][..3]);
        }
        // Edge 0–2 is the third midpoint of call 0's first face and the
        // first midpoint of call 1's first face.
        assert_eq!(calls[0].data[4 * 6][..3], calls[1].data[4 * 4][..3]);
    }

    #[test]
    fn open_mesh_keeps_its_boundary_vertices() {
        let mut calls = [call(&[[0, 1, 2]]), call(&[[0, 2, 3]])];
        let original = calls.clone();
        let stats = smooth_group(&mut calls, 0.7).unwrap();
        assert_eq!((stats.source_triangles, stats.triangles), (2, 8));
        for (call, old) in calls.iter().zip(&original) {
            for i in 0..4 {
                assert_eq!(call.data[4 * i][..3], old.data[4 * i][..3]);
            }
        }
    }

    #[test]
    fn one_call_can_contain_a_complete_open_patch() {
        let mut calls = [call(&[[0, 1, 2], [0, 2, 3]])];
        let stats = smooth_group(&mut calls, 0.7).unwrap();
        assert_eq!((stats.source_triangles, stats.triangles), (2, 8));
        assert_eq!(calls[0].indices.len(), 24);
    }

    #[test]
    fn non_manifold_mesh_stays_unchanged() {
        let mut calls = [call(&[[0, 1, 2], [0, 2, 3]]), call(&[[0, 1, 2]])];
        let original = calls.clone();
        assert!(smooth_group(&mut calls, 0.7).is_none());
        assert_eq!(calls, original);
    }

    #[test]
    fn cached_topology_reuses_source_mesh_at_a_new_camera_matrix() {
        let original = [call(&[[0, 1, 2], [0, 3, 1]]), call(&[[0, 2, 3], [1, 3, 2]])];
        let mut cache = Cache::default();
        cache.begin_frame(0.7, 0.0);
        for mut call in original.clone() {
            assert!(!cache.transform(7, &mut call));
        }
        assert_eq!(cache.finish_frame().len(), 1);
        cache.begin_frame(0.7, 0.0);
        let mut reused = original.clone();
        for call in &mut reused {
            call.uniforms[0][0] = 1.25f32.to_bits();
            assert!(cache.transform(8, call));
        }
        let mut expected = original;
        smooth_group(&mut expected, 0.7).unwrap();
        for (actual, expected) in reused.iter().zip(&expected) {
            assert_eq!(actual.data, expected.data);
            assert_eq!(actual.indices, expected.indices);
        }
        assert!(cache.finish_frame().is_empty());
    }

    #[test]
    fn cache_leaves_single_call_group_native() {
        let original = call(&[[0, 1, 2], [0, 2, 3]]);
        let mut cache = Cache::default();
        for _ in 0..2 {
            cache.begin_frame(0.7, 0.0);
            let mut actual = original.clone();
            assert!(!cache.transform(7, &mut actual));
            assert_eq!(actual, original);
            assert!(cache.finish_frame().is_empty());
        }
    }

    #[test]
    fn rock_projection_treats_steep_faces_as_surfaces() {
        let mut wall = call(&[[0, 1, 3]]);
        // This triangle lies in XZ, so its second texture coordinate must
        // follow height instead of the flat Y that the XY projection sees.
        assert!(tile_call(&mut wall, 2.0));
        assert_eq!(wall.indices, [0, 1, 2]);
        assert_eq!(wall.uniforms[4][1], 3);
        assert_eq!(wall.data[1][1], 8192);
        assert_eq!(wall.data[5][1], 0);
        assert_eq!(wall.data[9][1], 0);
        assert_eq!(position(&wall.data[0]), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn rock_projection_is_ready_on_the_first_frame() {
        let mut cache = Cache::default();
        cache.begin_frame(0.0, 2.0);
        let mut first = call(&[[0, 1, 3]]);
        assert!(cache.transform(3, &mut first));
        assert_eq!(first.data[1][1], 8192);
        cache.finish_frame();
        cache.begin_frame(0.0, 2.0);
        let mut second = call(&[[0, 1, 3]]);
        assert!(cache.transform(3, &mut second));
        assert_eq!(second, first);
    }
}
