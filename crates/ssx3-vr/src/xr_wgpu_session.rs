//! OpenXR stereo target and the matching offline world/HUD compositor pass.
//! Both use the same wgpu device and queue as the native game renderer.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ash::vk::{self, Handle};
use openxr as xr;
use wgpu::hal::{api::Vulkan, vulkan::TextureMemory};

use crate::xr_wgpu::WgpuXr;

pub const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
const TEST_FRAMES: u32 = 144;

/// CPU-side OpenXR handoff stages. `gpu_wait_ms` is a queue completion wait,
/// not an isolated measurement of either eye's GPU render time.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlitTiming {
    pub queue_wait_ms: f64,
    pub acquire_wait_ms: f64,
    pub encode_submit_ms: f64,
    pub gpu_wait_ms: f64,
    pub release_ms: f64,
}

#[derive(Clone, Copy)]
pub struct PanelEye {
    pub game_from_eye: [[f32; 4]; 4],
    /// Projection X/Y scales followed by X/Y offsets.
    pub clip: [f32; 4],
}

#[derive(Clone, Copy)]
pub struct PanelConfig {
    /// Menu card dimensions.
    pub width: f32,
    pub height: f32,
    /// Race HUD dimensions; its original art sits near the texture edges.
    pub hud_width: f32,
    pub hud_height: f32,
    pub distance: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PanelUniform {
    game_from_eye: [[f32; 4]; 4],
    clip: [f32; 4],
    size_distance_height: [f32; 4],
    world_valid: [f32; 4],
}

pub struct StereoTarget {
    // Textures are destroyed before the swapchain they refer to.
    textures: Vec<wgpu::Texture>,
    swapchain: Arc<Mutex<xr::Swapchain<xr::Vulkan>>>,
    width: u32,
    height: u32,
    panel: PanelPass,
}

/// The OpenXR and offline paths use the same final stereo/HUD shader.
struct PanelPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    uniforms: [wgpu::Buffer; 2],
}

impl PanelPass {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SSX 3 VR game image layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PanelUniform>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SSX 3 VR game image sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SSX 3 VR game image"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vr_panel.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SSX 3 VR game image pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("SSX 3 VR game image pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = std::array::from_fn(|eye| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if eye == 0 {
                    "SSX 3 VR left UI panel"
                } else {
                    "SSX 3 VR right UI panel"
                }),
                size: std::mem::size_of::<PanelUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        Self {
            layout,
            pipeline,
            sampler,
            uniforms,
        }
    }

    fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        sources: Option<[&wgpu::TextureView; 2]>,
        hud: &wgpu::TextureView,
        transmission: &wgpu::TextureView,
        panel_eyes: [PanelEye; 2],
        panel: PanelConfig,
    ) {
        for eye in 0..2 {
            let uniform = PanelUniform {
                game_from_eye: panel_eyes[eye].game_from_eye,
                clip: panel_eyes[eye].clip,
                size_distance_height: [panel.width, panel.height, panel.distance, panel.y],
                world_valid: [
                    if sources.is_some() { 1.0 } else { 0.0 },
                    panel.hud_width,
                    panel.hud_height,
                    0.0,
                ],
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &uniform as *const PanelUniform as *const u8,
                    std::mem::size_of::<PanelUniform>(),
                )
            };
            queue.write_buffer(&self.uniforms[eye], 0, bytes);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SSX 3 VR eye scene"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            sources.map_or(hud, |s| s[eye]),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(hud),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(transmission),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.uniforms[eye].as_entire_binding(),
                    },
                ],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: eye as u32,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("SSX 3 VR game image eye"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Synthetic swapchain target for measuring the actual stereo/HUD pass
/// without opening SteamVR.
pub struct OfflineStereoTarget {
    texture: wgpu::Texture,
    panel: PanelPass,
    width: u32,
    height: u32,
}

impl OfflineStereoTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SSX 3 VR offline stereo compositor"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            texture,
            panel: PanelPass::new(device, format),
            width,
            height,
        }
    }

    /// Uses the live shader and target extent, then waits for its submission
    /// just as the OpenXR handoff does. Swapchain waits and the compositor's
    /// own work are still absent from this measurement.
    pub fn blit_stereo(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sources: Option<[&wgpu::TextureView; 2]>,
        hud: &wgpu::TextureView,
        transmission: &wgpu::TextureView,
        panel_eyes: [PanelEye; 2],
        panel: PanelConfig,
    ) -> Result<f64, Box<dyn Error>> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SSX 3 VR offline stereo blit"),
        });
        self.panel.encode(
            device,
            queue,
            &mut encoder,
            &self.texture,
            sources,
            hud,
            transmission,
            panel_eyes,
            panel,
        );
        let submitted = queue.submit(Some(encoder.finish()));
        let started = Instant::now();
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: None,
        })?;
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    /// The same offline blit with GPU completion waited outside the Vulkan
    /// queue gate. The published source textures stay alive in the caller's
    /// VR snapshot until this method returns.
    pub fn blit_stereo_with_queue_access(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        access: &Mutex<()>,
        sources: Option<[&wgpu::TextureView; 2]>,
        hud: &wgpu::TextureView,
        transmission: &wgpu::TextureView,
        panel_eyes: [PanelEye; 2],
        panel: PanelConfig,
    ) -> Result<(f64, f64), Box<dyn Error>> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SSX 3 VR offline staged stereo blit"),
        });
        self.panel.encode(
            device,
            queue,
            &mut encoder,
            &self.texture,
            sources,
            hud,
            transmission,
            panel_eyes,
            panel,
        );
        let commands = encoder.finish();
        let lock_start = Instant::now();
        let submitted = {
            let _queue = access.lock().unwrap();
            let lock_wait_ms = lock_start.elapsed().as_secs_f64() * 1000.0;
            (queue.submit(Some(commands)), lock_wait_ms)
        };
        let started = Instant::now();
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted.0),
            timeout: None,
        })?;
        Ok((submitted.1, started.elapsed().as_secs_f64() * 1000.0))
    }

    /// Read the two *composited* layers, including the spatial HUD. This
    /// catches an eye that is blank after the last-stage shader even when the
    /// source eye textures are correct, and permits visual inspection.
    pub fn read_eyes(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<[(u32, u32, Vec<u32>); 2], Box<dyn Error>> {
        let row = (self.width * 4).next_multiple_of(256);
        let layer_bytes = row as u64 * self.height as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SSX 3 VR offline compositor readback"),
            size: layer_bytes * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 2,
            },
        );
        queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely())?;
        let bytes = slice.get_mapped_range()?;
        let mut eyes: [Vec<u32>; 2] =
            std::array::from_fn(|_| Vec::with_capacity((self.width * self.height) as usize));
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let offset = y * row as usize + x * 4;
                for (eye, pixels) in eyes.iter_mut().enumerate() {
                    let start = eye * layer_bytes as usize + offset;
                    pixels.push(u32::from_le_bytes(bytes[start..start + 4].try_into()?));
                }
            }
        }
        Ok(eyes.map(|pixels| (self.width, self.height, pixels)))
    }
}

impl StereoTarget {
    /// Recommended OpenXR image extent for each eye.
    pub fn eye_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn new(
        xr_instance: &xr::Instance,
        system: xr::SystemId,
        session: &xr::Session<xr::Vulkan>,
        device: &wgpu::Device,
    ) -> Result<Self, Box<dyn Error>> {
        let views = xr_instance.enumerate_view_configuration_views(system, VIEW_TYPE)?;
        if views.len() != 2 || views[0] != views[1] {
            return Err("Expected two equal-sized OpenXR stereo views".into());
        }
        let width = views[0].recommended_image_rect_width;
        let height = views[0].recommended_image_rect_height;
        let supported = session.enumerate_swapchain_formats()?;
        let (vk_format, format) = [
            (
                vk::Format::R8G8B8A8_SRGB,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ),
            (
                vk::Format::B8G8R8A8_SRGB,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ),
            (vk::Format::R8G8B8A8_UNORM, wgpu::TextureFormat::Rgba8Unorm),
            (vk::Format::B8G8R8A8_UNORM, wgpu::TextureFormat::Bgra8Unorm),
        ]
        .into_iter()
        .find(|(vk_format, _)| supported.contains(&(vk_format.as_raw() as _)))
        .ok_or("No usable OpenXR color format")?;
        let swapchain = Arc::new(Mutex::new(session.create_swapchain(
            &xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
                format: vk_format.as_raw() as _,
                sample_count: 1,
                width,
                height,
                face_count: 1,
                array_size: 2,
                mip_count: 1,
            },
        )?));
        let images = swapchain
            .lock()
            .map_err(|_| "OpenXR swapchain lock poisoned")?
            .enumerate_images()?;
        if images.is_empty() {
            return Err("OpenXR returned no swapchain images".into());
        }
        let hal_device =
            unsafe { device.as_hal::<Vulkan>() }.ok_or("OpenXR wgpu device is not Vulkan")?;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 2,
        };
        let textures = images
            .into_iter()
            .map(|image| {
                let hal_desc = wgpu::hal::TextureDescriptor {
                    label: Some("SSX 3 VR OpenXR color"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUses::COLOR_TARGET,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: Vec::new(),
                };
                // OpenXR owns the image and its memory. The callback prevents
                // wgpu-hal from destroying it and keeps the swapchain alive.
                let keep_alive = Arc::clone(&swapchain);
                let hal_texture = unsafe {
                    hal_device.texture_from_raw(
                        vk::Image::from_raw(image),
                        &hal_desc,
                        Some(Box::new(move || drop(keep_alive))),
                        TextureMemory::External,
                    )
                };
                let desc = wgpu::TextureDescriptor {
                    label: Some("SSX 3 VR OpenXR color"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                };
                // Contents are discarded on first use, so the initial Vulkan
                // layout is deliberately left undefined.
                unsafe {
                    device.create_texture_from_hal::<Vulkan>(
                        hal_texture,
                        &desc,
                        wgpu::TextureUses::UNINITIALIZED,
                    )
                }
            })
            .collect::<Vec<_>>();
        let panel = PanelPass::new(device, format);
        println!(
            "OpenXR wgpu swapchain: {width}x{height}x2, {vk_format:?}, {} images",
            textures.len()
        );
        Ok(Self {
            textures,
            swapchain,
            width,
            height,
            panel,
        })
    }

    pub fn clear_eyes(&self, graphics: &WgpuXr) -> Result<(), Box<dyn Error>> {
        let mut swapchain = self
            .swapchain
            .lock()
            .map_err(|_| "OpenXR swapchain lock poisoned")?;
        let index = swapchain.acquire_image()? as usize;
        swapchain.wait_image(xr::Duration::INFINITE)?;
        let texture = self
            .textures
            .get(index)
            .ok_or("OpenXR image index out of range")?;
        let mut encoder = graphics
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SSX 3 VR eye clears"),
            });
        for eye in 0..2 {
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: eye as u32,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let color = if eye == 0 {
                wgpu::Color {
                    r: 0.16,
                    g: 0.02,
                    b: 0.02,
                    a: 1.0,
                }
            } else {
                wgpu::Color {
                    r: 0.02,
                    g: 0.02,
                    b: 0.16,
                    a: 1.0,
                }
            };
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("SSX 3 VR eye clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let submitted = graphics.queue.submit(Some(encoder.finish()));
        // OpenXR may read the image immediately after release. Wait for this
        // diagnostic submission so the compositor never sees partial clears.
        graphics.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: None,
        })?;
        swapchain.release_image()?;
        Ok(())
    }

    /// Copy the two separately rendered scene images to their OpenXR layers.
    pub fn blit_stereo(
        &self,
        graphics: &WgpuXr,
        sources: Option<[&wgpu::TextureView; 2]>,
        hud: &wgpu::TextureView,
        transmission: &wgpu::TextureView,
        panel_eyes: [PanelEye; 2],
        panel: PanelConfig,
    ) -> Result<BlitTiming, Box<dyn Error>> {
        let acquire_start = Instant::now();
        let mut swapchain = self
            .swapchain
            .lock()
            .map_err(|_| "OpenXR swapchain lock poisoned")?;
        let index = swapchain.acquire_image()? as usize;
        swapchain.wait_image(xr::Duration::INFINITE)?;
        let encode_start = Instant::now();
        let texture = self
            .textures
            .get(index)
            .ok_or("OpenXR image index out of range")?;
        let mut encoder = graphics
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SSX 3 VR game image blit"),
            });
        self.panel.encode(
            &graphics.device,
            &graphics.queue,
            &mut encoder,
            texture,
            sources,
            hud,
            transmission,
            panel_eyes,
            panel,
        );
        let submitted = graphics.queue.submit(Some(encoder.finish()));
        let gpu_start = Instant::now();
        graphics.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: None,
        })?;
        let release_start = Instant::now();
        swapchain.release_image()?;
        Ok(BlitTiming {
            queue_wait_ms: 0.0,
            acquire_wait_ms: (encode_start - acquire_start).as_secs_f64() * 1000.0,
            encode_submit_ms: (gpu_start - encode_start).as_secs_f64() * 1000.0,
            gpu_wait_ms: (release_start - gpu_start).as_secs_f64() * 1000.0,
            release_ms: release_start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Synchronize only operations that may use the OpenXR Vulkan queue.
    /// The swapchain wait and wgpu submission-fence wait may block while the
    /// game's renderer prepares its next scene. The caller keeps the source
    /// snapshot alive through this method and still gates xrEndFrame.
    pub fn blit_stereo_with_queue_access(
        &self,
        graphics: &WgpuXr,
        access: &Mutex<()>,
        sources: Option<[&wgpu::TextureView; 2]>,
        hud: &wgpu::TextureView,
        transmission: &wgpu::TextureView,
        panel_eyes: [PanelEye; 2],
        panel: PanelConfig,
    ) -> Result<BlitTiming, Box<dyn Error>> {
        let acquire_start = Instant::now();
        let mut swapchain = self
            .swapchain
            .lock()
            .map_err(|_| "OpenXR swapchain lock poisoned")?;
        let mut queue_wait_ms = 0.0;
        let t = Instant::now();
        let index = {
            let _queue = access.lock().unwrap();
            queue_wait_ms += t.elapsed().as_secs_f64() * 1000.0;
            swapchain.acquire_image()? as usize
        };
        swapchain.wait_image(xr::Duration::INFINITE)?;
        let encode_start = Instant::now();
        let texture = self
            .textures
            .get(index)
            .ok_or("OpenXR image index out of range")?;
        let mut encoder = graphics
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SSX 3 VR staged game image blit"),
            });
        self.panel.encode(
            &graphics.device,
            &graphics.queue,
            &mut encoder,
            texture,
            sources,
            hud,
            transmission,
            panel_eyes,
            panel,
        );
        let commands = encoder.finish();
        let t = Instant::now();
        let submitted = {
            let _queue = access.lock().unwrap();
            queue_wait_ms += t.elapsed().as_secs_f64() * 1000.0;
            graphics.queue.submit(Some(commands))
        };
        let gpu_start = Instant::now();
        graphics.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: None,
        })?;
        let release_start = Instant::now();
        let t = Instant::now();
        {
            let _queue = access.lock().unwrap();
            queue_wait_ms += t.elapsed().as_secs_f64() * 1000.0;
            swapchain.release_image()?;
        }
        Ok(BlitTiming {
            queue_wait_ms,
            acquire_wait_ms: (encode_start - acquire_start).as_secs_f64() * 1000.0,
            encode_submit_ms: (gpu_start - encode_start).as_secs_f64() * 1000.0,
            gpu_wait_ms: (release_start - gpu_start).as_secs_f64() * 1000.0,
            release_ms: release_start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    pub fn submit_frame(
        &self,
        stream: &mut xr::FrameStream<xr::Vulkan>,
        display_time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        local: &xr::Space,
        views: &[xr::View],
    ) -> Result<(), Box<dyn Error>> {
        if views.len() != 2 {
            return Err(format!("Expected 2 OpenXR views, got {}", views.len()).into());
        }
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: self.width as _,
                height: self.height as _,
            },
        };
        let swapchain = self
            .swapchain
            .lock()
            .map_err(|_| "OpenXR swapchain lock poisoned")?;
        let layer_views: [xr::CompositionLayerProjectionView<'_, xr::Vulkan>; 2] =
            std::array::from_fn(|eye| {
                xr::CompositionLayerProjectionView::new()
                    .pose(views[eye].pose)
                    .fov(views[eye].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&swapchain)
                            .image_array_index(eye as u32)
                            .image_rect(rect),
                    )
            });
        let layer = xr::CompositionLayerProjection::new()
            .space(local)
            .views(&layer_views);
        stream.end(display_time, blend, &[&layer])?;
        Ok(())
    }
}

pub fn create_session(
    xr: &xr::Instance,
    system: xr::SystemId,
    graphics: &WgpuXr,
) -> Result<
    (
        xr::Session<xr::Vulkan>,
        xr::FrameWaiter,
        xr::FrameStream<xr::Vulkan>,
    ),
    Box<dyn Error>,
> {
    let hal_instance = unsafe { graphics.instance.as_hal::<Vulkan>() }
        .ok_or("OpenXR wgpu instance is not Vulkan")?;
    let hal_device =
        unsafe { graphics.device.as_hal::<Vulkan>() }.ok_or("OpenXR wgpu device is not Vulkan")?;
    let create = xr::vulkan::SessionCreateInfo {
        instance: hal_instance
            .shared_instance()
            .raw_instance()
            .handle()
            .as_raw() as _,
        physical_device: hal_device.raw_physical_device().as_raw() as _,
        device: hal_device.raw_device().handle().as_raw() as _,
        queue_family_index: hal_device.queue_family_index(),
        queue_index: 0,
    };
    // The OpenXR session may hold these raw handles beyond this function's
    // local borrows. Its guard keeps the backing wgpu objects alive.
    let guard = Box::new((graphics.instance.clone(), graphics.device.clone()));
    Ok(unsafe { xr.create_session_with_guard::<xr::Vulkan>(system, &create, guard)? })
}

#[cfg(test)]
mod panel_tests {
    #[test]
    fn spatial_hud_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(include_str!("vr_panel.wgsl"))
            .expect("VR compositing shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("VR compositing shader validates");
    }
}

pub fn probe(
    xr: &xr::Instance,
    system: xr::SystemId,
    graphics: &WgpuXr,
) -> Result<(), Box<dyn Error>> {
    let (session, mut waiter, mut stream) = create_session(xr, system, graphics)?;
    let local =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let blend = xr.enumerate_environment_blend_modes(system, VIEW_TYPE)?[0];
    let target = StereoTarget::new(xr, system, &session, &graphics.device)?;
    let mut events = xr::EventDataBuffer::new();
    let mut started = false;
    let mut frames = 0;
    let mut submitted = 0;
    let mut visible = false;
    let mut visible_period_ns = None;
    let mut requested_exit = false;
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        while let Some(event) = xr.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(change) => {
                    println!("OpenXR wgpu session: {:?}", change.state());
                    visible = matches!(
                        change.state(),
                        xr::SessionState::VISIBLE | xr::SessionState::FOCUSED
                    );
                    match change.state() {
                        xr::SessionState::READY => {
                            session.begin(VIEW_TYPE)?;
                            started = true;
                        }
                        xr::SessionState::STOPPING => {
                            session.end()?;
                            started = false;
                            if requested_exit {
                                println!("wgpu stereo frames submitted: {submitted}/{frames}");
                                if let Some(ns) = visible_period_ns {
                                    println!(
                                        "Visible display period: {:.3} ms ({:.1} Hz)",
                                        ns as f64 / 1_000_000.0,
                                        1_000_000_000.0 / ns as f64
                                    );
                                } else {
                                    println!("No visible headset frames were observed; visual confirmation is pending.");
                                }
                                return Ok(());
                            }
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => return Ok(()),
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => return Ok(()),
                _ => {}
            }
        }
        if !started {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let frame = waiter.wait()?;
        stream.begin()?;
        let (flags, views) =
            session.locate_views(VIEW_TYPE, frame.predicted_display_time, &local)?;
        if frames == 0 {
            println!(
                "wgpu display period: {:.3} ms ({:.1} Hz), pose flags: {:?}, views: {}",
                frame.predicted_display_period.as_nanos() as f64 / 1_000_000.0,
                1_000_000_000.0 / frame.predicted_display_period.as_nanos() as f64,
                flags,
                views.len()
            );
            for (eye, view) in views.iter().enumerate() {
                println!("Eye {eye} pose: {:?}, fov: {:?}", view.pose, view.fov);
            }
        }
        if frame.should_render {
            target.clear_eyes(graphics)?;
            submitted += 1;
            if visible {
                visible_period_ns = Some(frame.predicted_display_period.as_nanos());
            }
            target.submit_frame(
                &mut stream,
                frame.predicted_display_time,
                blend,
                &local,
                &views,
            )?;
        } else {
            stream.end(frame.predicted_display_time, blend, &[])?;
        }
        frames += 1;
        if frames == TEST_FRAMES && !requested_exit {
            session.request_exit()?;
            requested_exit = true;
        }
    }
    if frames < TEST_FRAMES {
        Err(
            format!("OpenXR wgpu session produced only {frames}/{TEST_FRAMES} frames in 12 s")
                .into(),
        )
    } else {
        Ok(())
    }
}
