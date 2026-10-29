//! Adopt the Vulkan instance and device created for OpenXR into wgpu 30.
//! This is the GPU context that the SSX 3 native renderer will use.
//!
//! The Vulkan/hal setup follows the MIT/Apache-2.0 `indite` interop example
//! (https://github.com/celphase/indite) and is adapted to this renderer.

use std::error::Error;
use std::ffi::CStr;

use ash::vk::{self, Handle};
use wgpu::hal::{api::Vulkan, Api, ExposedAdapter};

/// Field order keeps queue and device alive until before their adapter and
/// Vulkan instance go away.
pub struct WgpuXr {
    pub queue: wgpu::Queue,
    pub device: wgpu::Device,
    pub adapter: wgpu::Adapter,
    pub instance: wgpu::Instance,
}

pub fn create(xr: &openxr::Instance, system: openxr::SystemId) -> Result<WgpuXr, Box<dyn Error>> {
    const API: u32 = vk::API_VERSION_1_2;
    let requirements = xr.graphics_requirements::<openxr::Vulkan>(system)?;
    let target = openxr::Version::new(1, 2, 0);
    if target < requirements.min_api_version_supported
        || target > requirements.max_api_version_supported
    {
        return Err(format!(
            "OpenXR Vulkan 1.2 is outside {}..={}",
            requirements.min_api_version_supported, requirements.max_api_version_supported
        )
        .into());
    }
    let entry = unsafe { ash::Entry::load()? };
    let flags = wgpu::InstanceFlags::empty();
    let extensions = <Vulkan as Api>::Instance::desired_extensions(&entry, API, flags)?;
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let vk_app = vk::ApplicationInfo::default().api_version(API);
    let create = vk::InstanceCreateInfo::default()
        .application_info(&vk_app)
        .enabled_extension_names(&names);
    // SAFETY: OpenXR merges its required extensions into this Vulkan
    // instance; the creation structures and entry pointer stay live here.
    let raw = unsafe {
        xr.create_vulkan_instance(
            system,
            std::mem::transmute(entry.static_fn().get_instance_proc_addr),
            &create as *const _ as *const _,
        )?
    }
    .map_err(|e| format!("OpenXR Vulkan instance: {}", vk::Result::from_raw(e)))?;
    let raw = unsafe { ash::Instance::load(entry.static_fn(), vk::Instance::from_raw(raw as _)) };
    // SAFETY: `raw` was made from `entry` with the extensions wgpu-hal
    // requested. From this point wgpu-hal owns both the Vulkan instance and
    // the entry; we do not destroy them ourselves.
    let hal = unsafe {
        <Vulkan as Api>::Instance::from_raw(
            entry,
            raw,
            API,
            0,
            None,
            extensions,
            flags,
            wgpu::MemoryBudgetThresholds::default(),
            false,
            None,
        )?
    };
    let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal) };
    let (adapter, device, queue) = create_device(xr, system, &instance)?;
    Ok(WgpuXr {
        queue,
        device,
        adapter,
        instance,
    })
}

fn create_device(
    xr: &openxr::Instance,
    system: openxr::SystemId,
    instance: &wgpu::Instance,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), Box<dyn Error>> {
    let hal =
        unsafe { instance.as_hal::<Vulkan>() }.ok_or("wgpu did not adopt a Vulkan instance")?;
    let shared = hal.shared_instance();
    let raw_instance = shared.raw_instance();
    // SAFETY: OpenXR created this Vulkan instance for `system` and selects
    // the physical device compatible with the active headset.
    let physical = vk::PhysicalDevice::from_raw(unsafe {
        xr.vulkan_graphics_device(system, raw_instance.handle().as_raw() as _)?
    } as _);
    let exposed = hal
        .expose_adapter(physical)
        .ok_or("wgpu cannot expose the OpenXR GPU")?;
    let features = wgpu::Features::MULTIVIEW
        | (exposed.features
            & (wgpu::Features::DUAL_SOURCE_BLENDING
                | wgpu::Features::DEPTH_CLIP_CONTROL
                | wgpu::Features::CLIP_DISTANCES));
    if !exposed.features.contains(wgpu::Features::MULTIVIEW) {
        return Err("The OpenXR GPU lacks wgpu multiview".into());
    }
    // wgpu-core's indirect draw validator stores buffer offsets in u32;
    // passing the Vulkan driver's enormous raw max_buffer_size panics.
    let limits = wgpu::Limits {
        max_multiview_view_count: 2,
        ..wgpu::Limits::default().using_resolution(exposed.capabilities.limits.clone())
    };
    let (family, extensions, raw_device) = create_raw_device(
        xr,
        system,
        shared.entry(),
        raw_instance,
        physical,
        &exposed,
        features,
    )?;
    let hints = wgpu::MemoryHints::default();
    // SAFETY: OpenXR created the device from this adapter with the exact
    // extensions, features and queue passed to wgpu-hal. It now owns it.
    let opened = unsafe {
        exposed.adapter.device_from_raw(
            raw_device,
            None,
            &extensions,
            features,
            &limits,
            &hints,
            family,
            0,
        )?
    };
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let desc = wgpu::DeviceDescriptor {
        label: Some("SSX 3 OpenXR wgpu"),
        required_features: features,
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: hints,
        trace: wgpu::Trace::default(),
    };
    let (device, queue) = unsafe { adapter.create_device_from_hal(opened, &desc)? };
    Ok((adapter, device, queue))
}

fn create_raw_device(
    xr: &openxr::Instance,
    system: openxr::SystemId,
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    exposed: &ExposedAdapter<Vulkan>,
    features: wgpu::Features,
) -> Result<(u32, Vec<&'static CStr>, ash::Device), Box<dyn Error>> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    let family = families
        .iter()
        .position(|info| info.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .ok_or("OpenXR Vulkan device has no graphics queue")? as u32;
    let extensions = exposed.adapter.required_device_extensions(features);
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let mut enabled = exposed
        .adapter
        .physical_device_features(&extensions, features);
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&names);
    let info = enabled.add_to_device_create(info);
    // SAFETY: the device belongs to the OpenXR-selected physical GPU and
    // uses the features and extensions requested by wgpu-hal.
    let raw = unsafe {
        xr.create_vulkan_device(
            system,
            std::mem::transmute(entry.static_fn().get_instance_proc_addr),
            physical.as_raw() as _,
            &info as *const _ as *const _,
        )?
    }
    .map_err(|e| format!("OpenXR Vulkan device: {}", vk::Result::from_raw(e)))?;
    let device = unsafe { ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(raw as _)) };
    Ok((family, extensions, device))
}
