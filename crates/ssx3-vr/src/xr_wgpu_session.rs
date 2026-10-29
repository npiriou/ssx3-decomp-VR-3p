//! Diagnostic OpenXR session rendered by the same wgpu Vulkan device that
//! will drive the native game. Each eye receives a different clear color.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ash::vk::{self, Handle};
use openxr as xr;
use wgpu::hal::{api::Vulkan, vulkan::TextureMemory};

use crate::xr_wgpu::WgpuXr;

pub const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
const TEST_FRAMES: u32 = 144;

pub struct StereoTarget {
    // Textures are destroyed before the swapchain they refer to.
    textures: Vec<wgpu::Texture>,
    swapchain: Arc<Mutex<xr::Swapchain<xr::Vulkan>>>,
    width: u32,
    height: u32,
    panel_layout: wgpu::BindGroupLayout,
    panel_pipeline: wgpu::RenderPipeline,
    panel_sampler: wgpu::Sampler,
}

impl StereoTarget {
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
        let panel_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            ],
        });
        let panel_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
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
            bind_group_layouts: &[Some(&panel_layout)],
            immediate_size: 0,
        });
        let panel_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        println!(
            "OpenXR wgpu swapchain: {width}x{height}x2, {vk_format:?}, {} images",
            textures.len()
        );
        Ok(Self {
            textures,
            swapchain,
            width,
            height,
            panel_layout,
            panel_pipeline,
            panel_sampler,
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
        sources: [&wgpu::TextureView; 2],
    ) -> Result<(), Box<dyn Error>> {
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
                label: Some("SSX 3 VR game image blit"),
            });
        for eye in 0..2 {
            let bind_group = graphics
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("SSX 3 VR eye scene"),
                    layout: &self.panel_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(sources[eye]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.panel_sampler),
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
            pass.set_pipeline(&self.panel_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let submitted = graphics.queue.submit(Some(encoder.finish()));
        graphics.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: None,
        })?;
        swapchain.release_image()?;
        Ok(())
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
