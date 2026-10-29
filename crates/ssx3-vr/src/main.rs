//! Report the active OpenXR runtime and headset capabilities before the
//! game's renderer is connected to OpenXR swapchains.

use std::error::Error;
use std::path::PathBuf;

fn load_entry() -> Result<openxr::Entry, Box<dyn Error>> {
    // SAFETY: these libraries must implement the OpenXR loader ABI. The
    // default is resolved by the OS; the fallback is the installed SteamVR
    // loader or a path explicitly supplied by the user.
    if let Ok(entry) = unsafe { openxr::Entry::load(&()) } {
        return Ok(entry);
    }
    let loader = std::env::var_os("SSX3_OPENXR_LOADER")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("ProgramFiles(x86)").map(|base| {
                PathBuf::from(base)
                    .join("Steam/steamapps/common/SteamVR/bin/win64/openxr_loader.dll")
            })
        })
        .ok_or("OpenXR loader not found; set SSX3_OPENXR_LOADER to openxr_loader.dll")?;
    if !loader.is_file() {
        return Err(format!("OpenXR loader missing at {}", loader.display()).into());
    }
    println!("OpenXR loader: {}", loader.display());
    // SAFETY: same OpenXR loader ABI requirement as above.
    Ok(unsafe { openxr::Entry::load_from(&loader, &()) }?)
}

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
    let instance = entry.create_instance(&app, &openxr::ExtensionSet::default(), &[], &())?;
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
    Ok(())
}
