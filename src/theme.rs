//! Omarchy theme → GTK CSS. Colours come from the active theme's
//! `colors.toml`; the stylesheet itself lives in `style.css`.

use std::collections::HashMap;
use std::path::PathBuf;

const STYLE: &str = include_str!("style.css");

/// carbonfox-ish fallback for systems without an Omarchy theme.
const FALLBACK: &[(&str, &str)] = &[
    ("background", "#161616"),
    ("dark_background", "#0c0c0c"),
    ("lighter_background", "#252525"),
    ("foreground", "#f2f4f8"),
    ("muted", "#6e6f70"),
    ("accent", "#00b2ff"),
    ("selection", "#2a2a2a"),
    ("red", "#ee5396"),
];

pub fn theme_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [".local/state/omarchy/current", ".config/omarchy/current"]
        .iter()
        .map(|p| home.join(p))
        .find(|p| p.join("theme/colors.toml").is_file())
}

pub struct Theme {
    colors: HashMap<String, String>,
}

impl Theme {
    pub fn load() -> Self {
        let text = theme_dir()
            .and_then(|d| std::fs::read_to_string(d.join("theme/colors.toml")).ok())
            .unwrap_or_default();
        Self::parse(&text)
    }

    /// Only flat `key = "#rrggbb"` lines matter; everything else is ignored.
    fn parse(text: &str) -> Self {
        let mut colors: HashMap<String, String> =
            FALLBACK.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches('"');
            let hex = value.strip_prefix('#').unwrap_or("");
            if matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                colors.insert(key.trim().to_string(), value.to_string());
            }
        }
        Theme { colors }
    }

    pub fn color(&self, name: &str) -> &str {
        self.colors.get(name).map_or("#808080", String::as_str)
    }

    pub fn css(&self) -> String {
        let mut css = String::new();
        for (name, _) in FALLBACK {
            css.push_str(&format!("@define-color om_{name} {};\n", self.color(name)));
        }
        css.push_str(STYLE);
        css
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors_and_falls_back() {
        let t = Theme::parse("mode = \"dark\"\n# c\naccent = \"#ff0000\"\nbroken = \"#xyz\"\n");
        assert_eq!(t.color("accent"), "#ff0000");
        assert_eq!(t.color("background"), "#161616");
        assert!(t.css().contains("@define-color om_accent #ff0000;"));
    }
}
