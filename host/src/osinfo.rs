//! /etc/os-release: distro id, name, logo icon name and color.

use std::fs;

#[derive(Clone, Debug, Default)]
pub struct OsInfo {
    pub id: String,
    pub name: String,
    pub logo: Option<String>,
    pub ansi_color: Option<String>,
}

pub fn parse(text: &str) -> OsInfo {
    let mut o = OsInfo::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match k.trim() {
            "ID" => o.id = v,
            "NAME" => o.name = v,
            "LOGO" => o.logo = Some(v),
            "ANSI_COLOR" => o.ansi_color = Some(v),
            _ => {}
        }
    }
    if o.name.is_empty() {
        o.name = if o.id.is_empty() { "Linux".into() } else { o.id.clone() };
    }
    o
}

pub fn read() -> OsInfo {
    let text = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    parse(&text)
}

/// Header title: "Fedora Linux" -> "FEDORA", "NixOS" -> "NIXOS".
pub fn short_title(o: &OsInfo) -> String {
    let first = o.name.split_whitespace().next().unwrap_or("LINUX");
    first.to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fedora() {
        let o = parse("NAME=\"Fedora Linux\"\nID=fedora\nANSI_COLOR=\"0;38;2;60;110;180\"\nLOGO=fedora-logo-icon\n");
        assert_eq!(o.id, "fedora");
        assert_eq!(o.logo.as_deref(), Some("fedora-logo-icon"));
        assert_eq!(short_title(&o), "FEDORA");
    }

    #[test]
    fn nixos() {
        let o = parse("NAME=NixOS\nID=nixos\nLOGO=\"nix-snowflake\"\nANSI_COLOR=\"0;38;2;126;186;228\"\n");
        assert_eq!(short_title(&o), "NIXOS");
        assert_eq!(o.logo.as_deref(), Some("nix-snowflake"));
    }
}
