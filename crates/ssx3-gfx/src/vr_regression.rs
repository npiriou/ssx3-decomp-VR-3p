//! Small GPU regressions for spatial UI composition and particle depth.
use super::*;

struct Empty;
impl Mem for Empty {
    fn qword(&self, _: u32) -> u128 {
        0
    }
}

#[test]
fn sky_gpu_meshes_have_no_eye_translation_or_near_plane_cut() {
    use ssx3_render::gpu::{GpuCall, Link, Program};
    let gpu = ssx3_ps2::gpu::context().expect("GPU regression device");
    let r = Renderer::new(gpu.device.clone(), gpu.queue.clone());
    let mut g = r.0.lock().unwrap();
    g.out_size = Some((128, 128));
    let projection = vr::projection(-1., 1., -1., 1., 1., 30000.);
    for distance in [0.5f32, 8.] {
        let mut call = GpuCall {
            program: Program::Mat3Mesh,
            tag: (1u128 << 46) | (4u128 << 47),
            uniforms: [[0; 4]; 32],
            data: vec![],
            indices: vec![0, 1, 2, 1, 3, 2],
            index_mode: 0,
            pre: vec![],
            link: Link::None,
            mesh_key: None,
            split: None,
            alpha_max: 128.,
        };
        for (i, column) in projection.into_iter().enumerate() {
            call.uniforms[i] = column.map(f32::to_bits);
        }
        call.uniforms[4] = [0, 4, 1, 3];
        call.uniforms[18][0] = 1.0f32.to_bits();
        for [x, y] in [[-0.5, -0.5], [0.5, -0.5], [-0.5, 0.5], [0.5, 0.5]] {
            call.data.extend([
                [x * distance, y * distance, distance, 0.]
                    .map(|v| ((v * 32768.).round() as i32) as u32),
                [0; 4],
                [220, 80, 40, 128],
                [0; 4],
            ]);
        }
        let mut gs = gs::Gs::new();
        gs.begin((128, 128), 0, 0);
        gs.ctx[0].test = 0x50000;
        gs.ctx[0].scissor = 127 << 16 | 127 << 48;
        gs.tag.sky = true;
        gs.gpu_draw(call);
        let mut geometry = gs.take_geometry();
        // Exercise the already-projected CPU/GIF path in the same image.
        for [x, y] in [[8., 8.], [25., 8.], [8., 25.]] {
            geometry.verts.push(gs::Vtx {
                pos: [x, y, 0.4],
                col: [20., 140., 80., 128.],
                ..Default::default()
            });
        }
        geometry.batches.push(gs::Batch {
            state: gs::DrawState {
                prim: 0,
                ctx: gs::Ctx {
                    test: 0x50000,
                    scissor: 127 << 16 | 127 << 48,
                    ..Default::default()
                },
                texa: 0,
                fogcol: 0,
                tex: None,
                target: 0,
            },
            tag: gs::Tag {
                sky: true,
                ..Default::default()
            },
            first: 0,
            count: 3,
            gpu: None,
        });
        let mut captures = Vec::new();
        for translation in [[-3.2, 0., 0.], [3.2, 0., 0.], [20., 15., -40.]] {
            let mut view = vr::IDENTITY;
            view[3][..3].copy_from_slice(&translation);
            let eye = vr::Eye {
                view_from_game: view,
                projection,
            };
            let normal = vr::reproject(eye, projection).unwrap();
            let sky = vr::sky_reproject(eye, projection).unwrap();
            g.vr_eye = Some((0, normal));
            g.vr_reprojections = [vec![Some(normal)], vec![Some(normal)]];
            g.vr_sky_reprojections = [vec![Some(sky)], vec![Some(sky)]];
            g.draw_built(
                &Empty,
                &geometry,
                &inputs(),
                &geometry.batches,
                false,
                false,
            );
            let (_, _, pixels) = g.read_draw_buffer().unwrap();
            assert!(
                pixels.iter().filter(|&&p| p & 0xffffff != 0).count() > 3000,
                "distance {distance}"
            );
            captures.push(pixels);
        }
        assert_eq!(captures[0], captures[1], "left/right sky at {distance}");
        assert_eq!(
            captures[0], captures[2],
            "head translation must not move the sky"
        );
    }
}

fn inputs() -> DrawFrameInputs {
    DrawFrameInputs {
        phase1_end: usize::MAX,
        display_fbp: None,
        zscale: 4294967296.0,
        frame_gm: 0,
        world_casters: None,
        painter_sun: None,
        flare_light: None,
        silhouette_rts: Default::default(),
    }
}

#[test]
fn ui_overlap_alpha_masks_and_additive_draws_keep_independent_transmission() {
    let gpu = ssx3_ps2::gpu::context().expect("GPU regression device");
    let r = Renderer::new(gpu.device.clone(), gpu.queue.clone());
    let mut g = r.0.lock().unwrap();
    g.vr_ui = true;
    g.begin_vr_output((32, 32), true);
    let mut geometry = gs::FrameGeometry::default();
    geometry.state.size = (32, 32);
    // Two half-transparent overlapping sprites, a GS alpha-only write, and
    // additive green. Stored GS alpha ends opaque; world transmission is 1/4.
    for (color, mask, alpha) in [
        ([128.0, 0.0, 0.0, 64.0], 0xff00_0000u32, 0x44),
        ([0.0, 0.0, 255.0, 64.0], 0xff00_0000, 0x44),
        ([0.0, 0.0, 0.0, 128.0], 0x00ff_ffff, 0x44),
        ([0.0, 64.0, 0.0, 64.0], 0xff00_0000, 0x48),
    ] {
        let first = geometry.verts.len() as u32;
        for [x, y] in [
            [0., 0.],
            [32., 0.],
            [0., 32.],
            [32., 0.],
            [32., 32.],
            [0., 32.],
        ] {
            geometry.verts.push(gs::Vtx {
                pos: [x, y, 0.5],
                col: color,
                ..Default::default()
            });
        }
        geometry.batches.push(gs::Batch {
            state: gs::DrawState {
                prim: 1 << 6,
                ctx: gs::Ctx {
                    alpha,
                    frame: (mask as u64) << 32,
                    zbuf: 1 << 32,
                    test: 0x30000,
                    scissor: 31 << 16 | 31 << 48,
                    ..Default::default()
                },
                texa: 0,
                fogcol: 0,
                tex: None,
                target: 0,
            },
            tag: gs::Tag {
                hud: true,
                ..Default::default()
            },
            first,
            count: 6,
            gpu: None,
        });
    }
    g.draw_built(
        &Empty,
        &geometry,
        &inputs(),
        &geometry.batches,
        false,
        false,
    );
    let (_, _, pixels) = g.read_draw_buffer().unwrap();
    let pixel = pixels[512 * 1024 + 512].to_le_bytes();
    for (got, expected) in pixel.into_iter().zip([32i16, 32, 128, 255]) {
        assert!((got as i16 - expected).abs() <= 1, "GS color {pixel:?}");
    }
    let texture = &g.vr_ui_slot.as_ref().unwrap().transmission;
    let buffer = g.dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI regression transmission"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.dev.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            origin: wgpu::Origin3d {
                x: 512,
                y: 512,
                z: 0,
            },
            ..texture.as_image_copy()
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit([enc.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    g.dev.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap();
    // IEEE half-float 0.25 in each RGB channel, despite GS alpha == 1.
    for c in 0..3 {
        assert_eq!(u16::from_le_bytes([bytes[c * 2], bytes[c * 2 + 1]]), 0x3400);
    }
}

#[test]
fn particle_stereo_disparity_tracks_distance_on_the_gpu() {
    use ssx3_render::gpu::{GpuCall, Link, Program};
    let gpu = ssx3_ps2::gpu::context().expect("GPU regression device");
    let r = Renderer::new(gpu.device.clone(), gpu.queue.clone());
    let mut g = r.0.lock().unwrap();
    g.out_size = Some((128, 128));
    let projection = vr::projection(-1., 1., -1., 1., 1., 30000.);
    let mut disparity = Vec::new();
    for distance in [10., 100.] {
        let clip = vr::transform(projection, [0., 0., distance, 1.]);
        let mut call = GpuCall {
            program: Program::Mat4Particles,
            tag: (1u128 << 46) | (4u128 << 47),
            uniforms: [[0; 4]; 32],
            data: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
            index_mode: 0,
            pre: vec![],
            link: Link::None,
            mesh_key: None,
            split: None,
            alpha_max: 128.0,
        };
        call.uniforms[4] = [64.0, -64.0, 0.0, 0.0].map(f32::to_bits);
        call.uniforms[5] = [63.5, 63.5, 1.0, 0.0].map(f32::to_bits);
        call.uniforms[6] = [0.0, 0.0, 4.0, 0.0].map(f32::to_bits);
        call.uniforms[10][0] = 1;
        call.data.extend([
            clip.map(f32::to_bits),
            [0; 4],
            [255., 128., 64., 128.].map(f32::to_bits),
            [4. * distance, 4. * distance, 0., 0.].map(f32::to_bits),
        ]);
        let mut gs = gs::Gs::new();
        gs.begin((128, 128), 0, 0);
        gs.ctx[0].test = 0x30000;
        gs.ctx[0].scissor = 127 << 16 | 127 << 48;
        gs.gpu_draw(call);
        let geometry = gs.take_geometry();
        assert!(!geometry.batches.is_empty());
        let mut centers = Vec::new();
        for eye_x in [-1., 1.] {
            let mut view = vr::IDENTITY;
            view[3][0] = -eye_x;
            let m = vr::reproject(
                vr::Eye {
                    view_from_game: view,
                    projection,
                },
                projection,
            )
            .unwrap();
            g.vr_eye = Some((0, m));
            g.vr_reprojections = [vec![Some(m)], vec![Some(m)]];
            g.draw_built(
                &Empty,
                &geometry,
                &inputs(),
                &geometry.batches,
                false,
                false,
            );
            let (_, _, pixels) = g.read_draw_buffer().unwrap();
            let xs: Vec<_> = pixels
                .iter()
                .enumerate()
                .filter(|(_, p)| **p & 0xffffff != 0)
                .map(|(i, _)| (i % 128) as f32)
                .collect();
            assert!(!xs.is_empty());
            let center = xs.iter().sum::<f32>() / xs.len() as f32;
            centers.push(center);
            // The optimized clip-distance path must also retain eye depth,
            // rather than overwrite it with the original GS sprite depth.
            let buffer = g.dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle depth regression"),
                size: 128 * 128 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = g.dev.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                    ..g.targets.as_ref().unwrap().depth.as_image_copy()
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(512),
                        rows_per_image: Some(128),
                    },
                },
                wgpu::Extent3d {
                    width: 128,
                    height: 128,
                    depth_or_array_layers: 1,
                },
            );
            g.queue.submit([enc.finish()]);
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            g.dev.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let bytes = buffer.slice(..).get_mapped_range().unwrap();
            let at = (64 * 128 + center.round() as usize) * 4;
            let depth = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let expected = 0.5 * (1.0 - clip[2] / clip[3]);
            assert!(
                (depth - expected).abs() < 1e-5,
                "{distance}: depth {depth}, expected {expected}"
            );
        }
        disparity.push(centers[0] - centers[1]);
    }
    assert!((disparity[0] - 12.8).abs() < 1.1, "near {disparity:?}");
    assert!((disparity[1] - 1.28).abs() < 1.1, "far {disparity:?}");
}
