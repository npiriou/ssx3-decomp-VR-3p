//! Texture packs: the game's textures dumped as PNG files, and replacement
//! images (e.g. upscaled) loaded in their place.
//!
//! A texture is named by what it decodes to: an FNV-1a hash of its pixel and
//! CLUT contents, format, TEX0 (size, PSM, CLUT mode) and TEXA, as 16 hex
//! digits. The same image always gets the same name, wherever and whenever
//! the game uploads it; one with an animated CLUT gets one name per CLUT.
//!
//! - `SSX3_TEX_DUMP=1` (or a directory): every texture decoded, once, as
//!   `textures/dump/<name>.png`.
//! - `textures/replace/<name>.png` (`SSX3_TEX_REPLACE=<dir>`, `0` = off):
//!   used instead of the texture, any size (keep the original's aspect ratio;
//!   texture coordinates are normalised), with a full mipmap chain.
//!
//! Alpha: the GS's is 0..0x80 (0x80 = opaque). The PNGs hold it ×2 (0xFF =
//! opaque), and replacements are scaled back.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::tex::{Decoded, TexKey};

/// The file name (without `.png`) of a texture.
pub fn name(k: &TexKey) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in [k.hash, k.format as u64, k.tex0, k.texa] {
        for b in v.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

pub struct Pack {
    dump: Option<PathBuf>,
    dumped: HashSet<String>,
    /// Replacement images by name.
    replace: HashMap<String, PathBuf>,
    /// VR pack images decoded before the game starts presenting frames.
    preloaded: HashMap<String, Arc<Vec<Decoded>>>,
    /// Small, course-specific VR replacements included in the executable.
    vr_replacements: bool,
}

const VR_GRAVITUDE_CHEVRONS: &[u8] =
    include_bytes!("../../../assets/vr-textures/d5c495a1196f376d.png");
const VR_GRAVITUDE_ROCK: &[u8] = include_bytes!("../../../assets/vr-textures/b971a7abaf011a26.png");
const VR_SNOW_JAM_ICE: &[u8] = include_bytes!("../../../assets/vr-textures/b5f233a48a7355bb.png");
const VR_BUILTINS: [(&str, &[u8]); 3] = [
    ("d5c495a1196f376d", VR_GRAVITUDE_CHEVRONS),
    ("b971a7abaf011a26", VR_GRAVITUDE_ROCK),
    ("b5f233a48a7355bb", VR_SNOW_JAM_ICE),
];

impl Pack {
    pub fn from_env() -> Pack {
        let dump = std::env::var("SSX3_TEX_DUMP")
            .ok()
            .filter(|v| !v.is_empty() && v != "0")
            .map(|v| {
                if v == "1" {
                    PathBuf::from("textures/dump")
                } else {
                    PathBuf::from(v)
                }
            });
        if let Some(d) = &dump {
            match std::fs::create_dir_all(d) {
                Ok(()) => eprintln!("[gfx] dumping textures to {}", d.display()),
                Err(e) => eprintln!("[gfx] cannot create {}: {e}", d.display()),
            }
        }
        let dir = std::env::var("SSX3_TEX_REPLACE").unwrap_or_else(|_| "textures/replace".into());
        let mut replace = HashMap::new();
        if dir != "0" {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            replace.insert(stem.to_ascii_lowercase(), p.clone());
                        }
                    }
                }
            }
            if !replace.is_empty() {
                eprintln!("[gfx] {} replacement textures in {dir}", replace.len());
            }
        }
        let vr_replacements =
            dir != "0" && std::env::var("SSX3_VR_TEXTURES").is_ok_and(|v| v == "1");
        let mut preloaded = HashMap::new();
        if vr_replacements && std::env::var("SSX3_VR_PRELOAD_TEXTURES").as_deref() != Ok("0") {
            const BUDGET: usize = 64 * 1024 * 1024;
            let mut used = 0usize;
            let mut names: Vec<_> = replace.keys().cloned().collect();
            names.sort();
            for name in names {
                let Some(path) = replace.get(&name) else {
                    continue;
                };
                let Ok((w, h, rgba)) = read_png(path) else {
                    continue;
                };
                if w > 4096 || h > 4096 {
                    continue;
                }
                let levels = decoded_mips(w, h, rgba);
                let bytes: usize = levels.iter().map(|d| d.rgba.len() * 4).sum();
                if used + bytes > BUDGET {
                    continue;
                }
                used += bytes;
                preloaded.insert(name, Arc::new(levels));
            }
            for (name, png) in VR_BUILTINS {
                if replace.contains_key(name) {
                    continue;
                }
                let Ok((w, h, rgba)) = decode_png(std::io::Cursor::new(png)) else {
                    continue;
                };
                let levels = decoded_mips(w, h, rgba);
                let bytes: usize = levels.iter().map(|d| d.rgba.len() * 4).sum();
                if used + bytes > BUDGET {
                    continue;
                }
                used += bytes;
                preloaded.insert(name.to_owned(), Arc::new(levels));
            }
            if !preloaded.is_empty() {
                eprintln!(
                    "[gfx] preloaded {} VR textures ({:.1} MiB decoded)",
                    preloaded.len(),
                    used as f64 / 1048576.0
                );
            }
        }
        Pack {
            dump,
            dumped: HashSet::new(),
            replace,
            preloaded,
            vr_replacements,
        }
    }

    /// Write `d` as `<name>.png` in the dump directory (once).
    pub fn dump(&mut self, k: &TexKey, d: &Decoded) {
        let Some(dir) = &self.dump else { return };
        let n = name(k);
        if !self.dumped.insert(n.clone()) {
            return;
        }
        let path = dir.join(format!("{n}.png"));
        if path.exists() {
            return;
        }
        let rgba: Vec<u8> = d
            .rgba
            .iter()
            .flat_map(|p| {
                let [r, g, b, a] = p.to_le_bytes();
                [r, g, b, (a as u32 * 2).min(255) as u8]
            })
            .collect();
        if let Err(e) = write_png(&path, d.width, d.height, &rgba) {
            eprintln!("[gfx] {}: {e}", path.display());
        }
    }

    /// The replacement for `k`, if there is one: its mipmap chain (GS alpha).
    pub fn load(&self, k: &TexKey) -> Option<Arc<Vec<Decoded>>> {
        let name = name(k);
        if let Some(levels) = self.preloaded.get(&name) {
            return Some(Arc::clone(levels));
        }
        let source = if let Some(path) = self.replace.get(&name) {
            read_png(path)
        } else if self.vr_replacements {
            let Some((_, png)) = VR_BUILTINS.iter().find(|(stem, _)| *stem == name) else {
                return None;
            };
            decode_png(std::io::Cursor::new(*png))
        } else {
            return None;
        };
        match source {
            Ok((w, h, rgba)) => Some(Arc::new(decoded_mips(w, h, rgba))),
            Err(e) => {
                eprintln!("[gfx] replacement {name}: {e}");
                None
            }
        }
    }
}

fn decoded_mips(w: u32, h: u32, rgba: Vec<u8>) -> Vec<Decoded> {
    let px: Vec<u32> = rgba
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], ((c[3] as u32 + 1) / 2) as u8]))
        .collect();
    mips(Decoded {
        width: w,
        height: h,
        rgba: px,
    })
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

/// A PNG as RGBA8 (grey, grey+alpha and RGB are expanded; 16-bit is cut to
/// 8; palettes are expanded by the decoder).
fn read_png(path: &std::path::Path) -> Result<(u32, u32, Vec<u8>), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    decode_png(std::io::BufReader::new(f))
}

fn decode_png(input: impl std::io::BufRead + std::io::Seek) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(input);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; rd.output_buffer_size().ok_or("image too large")?];
    let info = rd.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG not expanded".into()),
    };
    Ok((w, h, rgba))
}

/// `d` and its mip levels down to 1×1 (2×2 box filter; odd sizes repeat the
/// edge).
pub fn mips(d: Decoded) -> Vec<Decoded> {
    let mut out = vec![d];
    loop {
        let p = out.last().unwrap();
        if p.width == 1 && p.height == 1 {
            break;
        }
        let (w, h) = ((p.width / 2).max(1), (p.height / 2).max(1));
        let at =
            |x: u32, y: u32| p.rgba[(y.min(p.height - 1) * p.width + x.min(p.width - 1)) as usize];
        let mut rgba = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let q = [
                    at(2 * x, 2 * y),
                    at(2 * x + 1, 2 * y),
                    at(2 * x, 2 * y + 1),
                    at(2 * x + 1, 2 * y + 1),
                ];
                let mut v = 0u32;
                for c in 0..4 {
                    let s: u32 = q.iter().map(|t| (t >> (8 * c)) & 0xff).sum();
                    v |= ((s + 2) / 4) << (8 * c);
                }
                rgba.push(v);
            }
        }
        out.push(Decoded {
            width: w,
            height: h,
            rgba,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_vr_chevrons_decode_with_translucent_ground() {
        let (w, h, rgba) = decode_png(std::io::Cursor::new(VR_GRAVITUDE_CHEVRONS)).unwrap();
        assert_eq!((w, h), (512, 512));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert!(rgba.chunks_exact(4).any(|p| p[3] < 128));
        assert!(rgba.chunks_exact(4).any(|p| p[3] == 255));
    }

    #[test]
    fn bundled_vr_rock_is_an_opaque_high_resolution_square() {
        let (w, h, rgba) = decode_png(std::io::Cursor::new(VR_GRAVITUDE_ROCK)).unwrap();
        assert_eq!((w, h), (1254, 1254));
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn bundled_vr_ice_keeps_the_native_mask() {
        let (w, h, rgba) = decode_png(std::io::Cursor::new(VR_SNOW_JAM_ICE)).unwrap();
        assert_eq!((w, h), (1254, 1254));
        let alpha = rgba.chunks_exact(4).map(|p| p[3]);
        assert_eq!(alpha.clone().min(), Some(0));
        assert_eq!(alpha.max(), Some(156));
    }

    #[test]
    fn mip_chain_halves_to_one_pixel() {
        let d = Decoded {
            width: 8,
            height: 2,
            rgba: vec![0x8040_2010; 16],
        };
        let m = mips(d);
        let sizes: Vec<(u32, u32)> = m.iter().map(|d| (d.width, d.height)).collect();
        assert_eq!(sizes, [(8, 2), (4, 1), (2, 1), (1, 1)]);
        assert!(m.iter().all(|d| d.rgba.iter().all(|&p| p == 0x8040_2010)));
    }

    #[test]
    fn png_round_trip_keeps_gs_alpha() {
        let dir = std::env::temp_dir().join(format!("ssx3-pack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let k = TexKey {
            info: 1,
            pixels: 2,
            clut: 3,
            format: 4,
            tex0: 5,
            texa: 6,
            hash: 7,
        };
        let d = Decoded {
            width: 2,
            height: 1,
            rgba: vec![0x8011_2233, 0x0044_5566],
        };
        let mut p = Pack {
            dump: Some(dir.clone()),
            dumped: HashSet::new(),
            replace: HashMap::new(),
            preloaded: HashMap::new(),
            vr_replacements: false,
        };
        p.dump(&k, &d);
        let file = dir.join(format!("{}.png", name(&k)));
        assert!(file.exists());
        p.replace.insert(name(&k), file);
        let back = p.load(&k).unwrap();
        assert_eq!(back[0].rgba, d.rgba);
        assert_eq!(back.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
