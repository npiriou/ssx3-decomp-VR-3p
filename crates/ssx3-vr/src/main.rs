//! Report the active OpenXR runtime and headset capabilities before the
//! game's renderer is connected to OpenXR swapchains.

use std::error::Error;

mod xr_session;
mod xr_swapchain;

use ssx3_vr::{load_entry, xr_wgpu, xr_wgpu_session};

fn main() -> Result<(), Box<dyn Error>> {
    let entry = load_entry()?;
    let extensions = entry.enumerate_extensions()?;
    let app = openxr::ApplicationInfo {
        application_name: "SSX 3 VR probe",
        application_version: 1,
        engine_name: "ssx3-decomp",
        engine_version: 1,
        ..Default::default()
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let with_session = args.iter().any(|arg| arg == "--session");
    let with_wgpu = args.iter().any(|arg| arg == "--wgpu");
    let with_wgpu_session = args.iter().any(|arg| arg == "--wgpu-session");
    let mut enabled = openxr::ExtensionSet::default();
    if with_session || with_wgpu || with_wgpu_session {
        if !extensions.khr_vulkan_enable2 {
            return Err("This OpenXR runtime lacks XR_KHR_vulkan_enable2".into());
        }
        enabled.khr_vulkan_enable2 = true;
    }
    let instance = entry.create_instance(&app, &enabled, &[], &())?;
    let runtime = instance.properties()?;
    println!(
        "OpenXR runtime: {} ({:?})",
        runtime.runtime_name, runtime.runtime_version
    );
    println!("Vulkan graphics binding: {}", extensions.khr_vulkan_enable2);

    let system = match instance.system(openxr::FormFactor::HEAD_MOUNTED_DISPLAY) {
        Ok(system) => system,
        Err(err) => {
            eprintln!("No active head-mounted display: {err:?}");
            return Ok(());
        }
    };
    let properties = instance.system_properties(system)?;
    println!("Headset: {}", properties.system_name);
    let views = instance.enumerate_view_configuration_views(
        system,
        openxr::ViewConfigurationType::PRIMARY_STEREO,
    )?;
    println!("Stereo views: {}", views.len());
    for (eye, view) in views.iter().enumerate() {
        println!(
            "Eye {eye}: recommended {}x{}, samples {} (maximum {}x{})",
            view.recommended_image_rect_width,
            view.recommended_image_rect_height,
            view.recommended_swapchain_sample_count,
            view.max_image_rect_width,
            view.max_image_rect_height,
        );
    }
    if with_session {
        xr_session::probe(&instance, system)?;
    }
    if with_wgpu || with_wgpu_session {
        let graphics = xr_wgpu::create(&instance, system)?;
        println!("OpenXR wgpu adapter: {}", graphics.adapter.get_info().name);
        if with_wgpu_session {
            xr_wgpu_session::probe(&instance, system, &graphics)?;
        }
        drop(graphics);
    }
    Ok(())
}
