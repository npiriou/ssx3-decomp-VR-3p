//! Minimal Vulkan-backed OpenXR session. It submits empty frames while
//! measuring the compositor's cadence and locating the two HMD views.
//! The game renderer will later share this graphics device and submit images.

use std::error::Error;
use std::time::{Duration, Instant};

use ash::vk::{self, Handle};
use openxr as xr;

use crate::xr_swapchain::XrColorTarget;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
const TEST_FRAMES: u32 = 144;

pub fn probe(instance: &xr::Instance, system: xr::SystemId) -> Result<(), Box<dyn Error>> {
    // OpenXR's Vulkan creation functions select the correct adapter and
    // enable the runtime's required extensions.
    let required = instance.graphics_requirements::<xr::Vulkan>(system)?;
    println!(
        "Vulkan range: {} to {}",
        required.min_api_version_supported, required.max_api_version_supported
    );
    let target = xr::Version::new(1, 1, 0);
    if target < required.min_api_version_supported || target > required.max_api_version_supported {
        return Err("Vulkan 1.1 is outside the runtime's supported range".into());
    }
    let vk_entry = unsafe { ash::Entry::load()? };
    let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
    let create = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: OpenXR provides a Vulkan instance creation function and the
    // pointers below remain live throughout the call.
    let raw_instance = unsafe {
        instance.create_vulkan_instance(
            system,
            std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
            &create as *const _ as *const _,
        )?
    }
    .map_err(|e| format!("Vulkan instance creation failed: {e}"))?;
    let vk_instance = unsafe {
        ash::Instance::load(
            vk_entry.static_fn(),
            vk::Instance::from_raw(raw_instance as _),
        )
    };

    let result = probe_with_instance(instance, system, &vk_entry, &vk_instance);
    unsafe { vk_instance.destroy_instance(None) };
    result
}

fn probe_with_instance(
    instance: &xr::Instance,
    system: xr::SystemId,
    vk_entry: &ash::Entry,
    vk_instance: &ash::Instance,
) -> Result<(), Box<dyn Error>> {
    // SAFETY: this instance was created by the OpenXR runtime for `system`.
    let physical = vk::PhysicalDevice::from_raw(unsafe {
        instance.vulkan_graphics_device(system, vk_instance.handle().as_raw() as _)
    }? as _);
    let families = unsafe { vk_instance.get_physical_device_queue_family_properties(physical) };
    let family = families
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .ok_or("The OpenXR Vulkan device has no graphics queue")? as u32;
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let create = vk::DeviceCreateInfo::default().queue_create_infos(&queues);
    // SAFETY: OpenXR uses the selected Vulkan physical device and live
    // creation structures to create a compatible graphics device.
    let raw_device = unsafe {
        instance.create_vulkan_device(
            system,
            std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
            physical.as_raw() as _,
            &create as *const _ as *const _,
        )?
    }
    .map_err(|e| format!("Vulkan device creation failed: {e}"))?;
    let device =
        unsafe { ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _)) };
    let result = probe_with_device(instance, system, vk_instance, physical, &device, family);
    unsafe { device.destroy_device(None) };
    result
}

fn probe_with_device(
    instance: &xr::Instance,
    system: xr::SystemId,
    vk_instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    device: &ash::Device,
    family: u32,
) -> Result<(), Box<dyn Error>> {
    let (session, mut waiter, mut stream) = unsafe {
        instance.create_session::<xr::Vulkan>(
            system,
            &xr::vulkan::SessionCreateInfo {
                instance: vk_instance.handle().as_raw() as _,
                physical_device: physical.as_raw() as _,
                device: device.handle().as_raw() as _,
                queue_family_index: family,
                queue_index: 0,
            },
        )?
    };
    let local =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let blend = instance.enumerate_environment_blend_modes(system, VIEW_TYPE)?[0];
    let mut target = XrColorTarget::new(instance, system, &session, device, family)?;
    let mut events = xr::EventDataBuffer::new();
    let mut started = false;
    let mut frames = 0u32;
    let mut submitted = 0u32;
    let mut visible = false;
    let mut visible_period_ns = None;
    let mut requested_exit = false;
    let deadline = Instant::now() + Duration::from_secs(12);

    while Instant::now() < deadline {
        while let Some(event) = instance.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(change) => {
                    println!("OpenXR session: {:?}", change.state());
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
                                println!(
                                    "Diagnostic stereo frames submitted: {submitted}/{frames}"
                                );
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
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            return Ok(());
                        }
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
                "Display period: {:.3} ms ({:.1} Hz), pose flags: {:?}, views: {}",
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
            target.clear_eyes()?;
            submitted += 1;
            if visible {
                visible_period_ns = Some(frame.predicted_display_period.as_nanos());
            }
            let layer_views = target.projection_views(&views)?;
            let layer = xr::CompositionLayerProjection::new()
                .space(&local)
                .views(&layer_views);
            stream.end(frame.predicted_display_time, blend, &[&layer])?;
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
        Err(format!("OpenXR session produced only {frames}/{TEST_FRAMES} frames in 12 s").into())
    } else {
        Ok(())
    }
}
