//! Distro logo: uses the logo files already installed on the system
//! (os-release LOGO= icon name), or a file the user points to.
//! Converted to 40x40 RGB565 with ImageMagick and blended onto the theme background.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::expand;
use crate::osinfo::OsInfo;
use crate::proto::Buf;
use crate::theme::{mix, rgb565, Rgb};

pub const SIZE: u32 = 40;

fn icon_dirs() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/usr/share"), PathBuf::from("/run/current-system/sw/share")];
    if let Ok(xdg) = std::env::var("XDG_DATA_DIRS") {
        roots.extend(xdg.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    let mut dirs = Vec::new();
    for r in roots {
        dirs.push(r.join("pixmaps"));
        for size in ["scalable", "256x256", "128x128", "96x96", "64x64", "48x48"] {
            dirs.push(r.join("icons/hicolor").join(size).join("apps"));
        }
    }
    dirs
}

/// Where is the logo for this system?
pub fn find(setting: &str, os: &OsInfo) -> Option<PathBuf> {
    match setting {
        "none" => return None,
        "auto" => {}
        path => {
            let p = expand(path);
            return p.exists().then_some(p);
        }
    }
    let mut names: Vec<String> = Vec::new();
    if let Some(l) = &os.logo {
        names.push(l.clone());
    }
    if !os.id.is_empty() {
        names.extend([format!("{}-logo-icon", os.id), format!("{}-logo", os.id), os.id.clone()]);
    }
    names.push("distributor-logo".into());
    for dir in icon_dirs() {
        for name in &names {
            for ext in ["svg", "png"] {
                let p = dir.join(format!("{name}.{ext}"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Rasterize to SIZE x SIZE RGBA bytes with ImageMagick (`magick`, or legacy `convert`).
fn rasterize(path: &Path) -> Option<Vec<u8>> {
    let geom = format!("{SIZE}x{SIZE}");
    for tool in ["magick", "convert"] {
        let out = Command::new(tool)
            .args(["-background", "none", "-density", "384"])
            .arg(path)
            .args(["-resize", &geom, "-gravity", "center", "-extent", &geom, "rgba:-"])
            .output();
        if let Ok(o) = out {
            if o.status.success() && o.stdout.len() == (SIZE * SIZE * 4) as usize {
                return Some(o.stdout);
            }
        }
    }
    None
}

/// LOGO packet: w, h, then w*h RGB565 pixels (alpha already blended onto `bg`).
/// w = 0 means "no logo".
pub fn payload(path: Option<&Path>, bg: Rgb) -> (Vec<u8>, String) {
    let Some(path) = path else {
        return (vec![0, 0], "no logo".into());
    };
    let Some(rgba) = rasterize(path) else {
        return (vec![0, 0], format!("cannot convert {} (install ImageMagick)", path.display()));
    };
    let mut b = Buf::default();
    b.u8(SIZE as u8).u8(SIZE as u8);
    for px in rgba.chunks_exact(4) {
        let c = mix(bg, (px[0], px[1], px[2]), px[3] as f32 / 255.0);
        b.u16(rgb565(c));
    }
    (b.0, path.display().to_string())
}
