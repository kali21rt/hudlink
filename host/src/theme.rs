//! Colors: presets, distro accent (os-release ANSI_COLOR), matugen file, cava gradient.

use std::fs;
use std::path::Path;

use crate::config::{expand, home, Config};
use crate::osinfo::OsInfo;
use crate::proto::Buf;

pub type Rgb = (u8, u8, u8);

/// The palette sent to the ESP32 (13 colors).
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub bg: Rgb,
    pub surface: Rgb,
    pub line: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub accent2: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
    pub hot: Rgb,
    pub grad: [Rgb; 3],
}

pub const PRESETS: [&str; 7] = ["ocean", "forest", "rose", "slate", "mono", "amber", "kayon"];

pub fn hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_matches(|c| c == '\'' || c == '"');
    let s = s.strip_prefix('#').or_else(|| s.strip_prefix("0x")).unwrap_or(s);
    if s.len() < 6 || !s.is_char_boundary(6) {
        return None;
    }
    let v = u32::from_str_radix(&s[..6], 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn h(v: u32) -> Rgb {
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

pub fn rgb565(c: Rgb) -> u16 {
    ((c.0 as u16 & 0xF8) << 8) | ((c.1 as u16 & 0xFC) << 3) | (c.2 as u16 >> 3)
}

const OK: u32 = 0x8FC79A;
const WARN: u32 = 0xE0B465;
const HOT: u32 = 0xE06C6C;

fn preset(name: &str) -> Option<Palette> {
    // bg, surface, line, text, dim, accent, accent2, grad0, grad1, grad2
    let p: [u32; 10] = match name {
        "ocean" => [0x0B0F14, 0x131A22, 0x223040, 0xDDE6EE, 0x7D8C9A, 0x6FA8DC, 0x8FD3C8, 0x2E4A6B, 0x6FA8DC, 0xA8D8F0],
        "forest" => [0x0C110D, 0x141C16, 0x24332A, 0xDDE8DE, 0x7E8F82, 0x8FBF7F, 0xC8D987, 0x2F4A33, 0x8FBF7F, 0xD6E6A8],
        "rose" => [0x130D10, 0x1D1418, 0x34242B, 0xF0E0E6, 0x9A8088, 0xE48AA6, 0xF0B8A0, 0x5A2E40, 0xE48AA6, 0xF6C6D4],
        "slate" => [0x0E0F12, 0x17191E, 0x2A2E36, 0xE2E4E9, 0x8A8F99, 0x9AA7FF, 0x7FD1E8, 0x3A3F70, 0x9AA7FF, 0xD0D6FF],
        "mono" => [0x000000, 0x111111, 0x2A2A2A, 0xEEEEEE, 0x888888, 0xFFFFFF, 0xBBBBBB, 0x444444, 0x999999, 0xFFFFFF],
        "amber" => [0x0D0A05, 0x17120A, 0x2E2414, 0xF2E6CF, 0x9C8C70, 0xF0B040, 0xE07A3A, 0x5A3A10, 0xF0B040, 0xFFE0A0],
        "kayon" => [0x0E0A07, 0x1A120C, 0x3A2A1E, 0xE6DAC4, 0x8C7B66, 0xD6BE50, 0xD6761E, 0x5A3418, 0xD6761E, 0xD6BE50],
        _ => return None,
    };
    Some(Palette {
        bg: h(p[0]),
        surface: h(p[1]),
        line: h(p[2]),
        text: h(p[3]),
        dim: h(p[4]),
        accent: h(p[5]),
        accent2: h(p[6]),
        ok: h(OK),
        warn: h(WARN),
        hot: h(HOT),
        grad: [h(p[7]), h(p[8]), h(p[9])],
    })
}

/// A calm dark palette built around one accent color.
pub fn from_accent(a: Rgb) -> Palette {
    let black = (0, 0, 0);
    let white = (255, 255, 255);
    let gray = (136, 136, 136);
    Palette {
        bg: mix(black, a, 0.06),
        surface: mix(black, a, 0.11),
        line: mix(mix(black, gray, 0.25), a, 0.18),
        text: mix(white, a, 0.10),
        dim: mix(gray, a, 0.25),
        accent: a,
        accent2: mix(a, white, 0.45),
        ok: h(OK),
        warn: h(WARN),
        hot: h(HOT),
        grad: [mix(black, a, 0.45), a, mix(a, white, 0.55)],
    }
}

/// os-release ANSI_COLOR: "0;38;2;60;110;180" (truecolor) or "0;34" (basic).
pub fn ansi_color(s: &str) -> Option<Rgb> {
    let parts: Vec<u8> = s.split(';').filter_map(|p| p.trim().parse().ok()).collect();
    if let Some(i) = parts.windows(2).position(|w| w == [38, 2]) {
        if parts.len() >= i + 5 {
            return Some((parts[i + 2], parts[i + 3], parts[i + 4]));
        }
    }
    parts.iter().rev().find_map(|&c| match c {
        31 | 91 => Some(h(0xE06C6C)),
        32 | 92 => Some(h(0x8FC79A)),
        33 | 93 => Some(h(0xE0B465)),
        34 | 94 => Some(h(0x6FA8DC)),
        35 | 95 => Some(h(0xC792EA)),
        36 | 96 => Some(h(0x7FD1E8)),
        _ => None,
    })
}

/// matugen output written by our template: `key = #rrggbb` lines.
pub fn read_matugen(path: &Path) -> Option<Palette> {
    let text = fs::read_to_string(path).ok()?;
    let get = |k: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(key, _)| key.trim() == k)
            .and_then(|(_, v)| hex(v))
    };
    let accent = get("accent")?;
    let mut p = from_accent(accent);
    let set = |slot: &mut Rgb, k: &str| {
        if let Some(c) = get(k) {
            *slot = c;
        }
    };
    set(&mut p.bg, "bg");
    set(&mut p.surface, "surface");
    set(&mut p.line, "line");
    set(&mut p.text, "text");
    set(&mut p.dim, "dim");
    set(&mut p.accent2, "accent2");
    set(&mut p.hot, "hot");
    set(&mut p.grad[0], "grad0");
    set(&mut p.grad[1], "grad1");
    set(&mut p.grad[2], "grad2");
    Some(p)
}

/// Gradient colors from the user's own cava config ([color] section).
pub fn read_cava_gradient() -> Option<[Rgb; 3]> {
    let text = fs::read_to_string(home().join(".config/cava/config")).ok()?;
    let mut grads: Vec<(u32, Rgb)> = Vec::new();
    let mut fg = None;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with(';') || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let k = k.trim();
        if let Some(n) = k.strip_prefix("gradient_color_") {
            if let (Ok(n), Some(c)) = (n.parse::<u32>(), hex(v)) {
                grads.push((n, c));
            }
        } else if k == "foreground" {
            fg = hex(v);
        }
    }
    grads.sort_by_key(|g| g.0);
    match grads.len() {
        0 => fg.map(|c| [mix((0, 0, 0), c, 0.45), c, mix(c, (255, 255, 255), 0.5)]),
        1 => Some([grads[0].1; 3]),
        n => Some([grads[0].1, grads[n / 2].1, grads[n - 1].1]),
    }
}

/// Resolve the palette from config. Returns (palette, human-readable source).
pub fn resolve(cfg: &Config, os: &OsInfo) -> (Palette, String) {
    let matugen = expand(&cfg.matugen_file);
    let distro = || os.ansi_color.as_deref().and_then(ansi_color).map(from_accent);
    let (mut pal, src) = match cfg.theme.as_str() {
        "matugen" => match read_matugen(&matugen) {
            Some(p) => (p, "matugen".to_string()),
            None => (preset("slate").unwrap(), format!("matugen file missing ({}), using slate", matugen.display())),
        },
        "distro" => match distro() {
            Some(p) => (p, format!("distro color ({})", os.id)),
            None => (preset("ocean").unwrap(), "no ANSI_COLOR in os-release, using ocean".into()),
        },
        "auto" => {
            if let Some(p) = read_matugen(&matugen) {
                (p, "matugen".into())
            } else if let Some(p) = distro() {
                (p, format!("distro color ({})", os.id))
            } else {
                (preset("ocean").unwrap(), "ocean".into())
            }
        }
        name => match preset(name) {
            Some(p) => (p, name.to_string()),
            None => (preset("ocean").unwrap(), format!("unknown theme \"{name}\", using ocean")),
        },
    };
    if let Some(a) = cfg.accent.as_deref().and_then(hex) {
        let derived = from_accent(a);
        pal.accent = a;
        pal.accent2 = derived.accent2;
        pal.grad = derived.grad;
    }
    if cfg.cava_colors == "cava" {
        if let Some(g) = read_cava_gradient() {
            pal.grad = g;
        }
    }
    (pal, src)
}

pub fn payload(p: &Palette) -> Vec<u8> {
    let mut b = Buf::default();
    for c in [p.bg, p.surface, p.line, p.text, p.dim, p.accent, p.accent2, p.ok, p.warn, p.hot, p.grad[0], p.grad[1], p.grad[2]] {
        b.u16(rgb565(c));
    }
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(hex("#3c6eb4"), Some((0x3c, 0x6e, 0xb4)));
        assert_eq!(hex("'#FFFFFF'"), Some((255, 255, 255)));
        assert_eq!(hex("nope"), None);
        assert_eq!(ansi_color("0;38;2;60;110;180"), Some((60, 110, 180)));
        assert_eq!(ansi_color("1;34"), Some(h(0x6FA8DC)));
        assert_eq!(rgb565((255, 255, 255)), 0xFFFF);
        assert_eq!(rgb565((255, 0, 0)), 0xF800);
    }

    #[test]
    fn matugen_file() {
        let dir = std::env::temp_dir().join("hudlink-test-matugen");
        fs::write(&dir, "accent = #a6c8ff\nbg = #111318\ngrad2 = #ffffff\n").unwrap();
        let p = read_matugen(&dir).unwrap();
        assert_eq!(p.accent, (0xa6, 0xc8, 0xff));
        assert_eq!(p.bg, (0x11, 0x13, 0x18));
        assert_eq!(p.grad[2], (255, 255, 255));
        assert_eq!(payload(&p).len(), 26);
    }

    #[test]
    fn every_preset_exists() {
        for n in PRESETS {
            assert!(preset(n).is_some(), "{n}");
        }
    }
}
