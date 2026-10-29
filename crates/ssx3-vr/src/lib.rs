//! OpenXR Vulkan/wgpu bridge shared by the diagnostic and game front ends.

use std::error::Error;
use std::path::PathBuf;

pub mod xr_wgpu;
pub mod xr_wgpu_session;
pub use openxr;

pub fn load_entry() -> Result<openxr::Entry, Box<dyn Error>> {
    // SAFETY: the selected library implements the OpenXR loader ABI.
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
    Ok(unsafe { openxr::Entry::load_from(&loader, &()) }?)
}
