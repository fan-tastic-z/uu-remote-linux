//! The active Omarchy theme as nsh theme overrides. Omarchy materializes the current theme's
//! palette at `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` and does not theme GTK apps,
//! so this window reads the palette itself and maps it onto nsh's shadcn tokens. One palette
//! serves both appearances (an Omarchy theme already matches its own `mode`); tokens left out
//! keep nsh's defaults. Colors are precomputed hex (no `var()`/`color-mix()` in the output).

use std::collections::HashMap;
use std::path::PathBuf;

/// cssVars JSON for `AppView::set_theme_overrides`, or `None` without an Omarchy theme
/// (nsh's default neutral theme then draws the window).
pub fn omarchy_overrides() -> Option<String> {
    overrides_from_colors(&std::fs::read_to_string(colors_toml()?).ok()?)
}

fn colors_toml() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(state.join("omarchy").join("current").join("theme").join("colors.toml"))
}

/// `key = "#rrggbb"` lines → a key→hex map; anything else is skipped.
fn parse_colors(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"');
        if value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
            out.insert(key.trim().to_owned(), value.to_owned());
        }
    }
    out
}

/// Linear mix of two `#rrggbb` colors toward `to` by `t` (0..1), as `#rrggbb`.
fn mix(from: &str, to: &str, t: f32) -> String {
    let channel = |a: u8, b: u8| a as f32 + (b as f32 - a as f32) * t;
    let (a, b) = (hex(from), hex(to));
    let (r, g, bch) = (
        channel(a[0], b[0]).round() as u8,
        channel(a[1], b[1]).round() as u8,
        channel(a[2], b[2]).round() as u8,
    );
    format!("#{r:02x}{g:02x}{bch:02x}")
}

fn hex(color: &str) -> [u8; 3] {
    let value = u32::from_str_radix(&color[1..], 16).unwrap_or(0xffffff);
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// The palette of `colors.toml` text as shadcn cssVars. `None` when the file has no
/// `background` (not a usable palette; the caller falls back to the default theme).
fn overrides_from_colors(text: &str) -> Option<String> {
    let colors = parse_colors(text);
    let c = |key: &str| colors.get(key).cloned();
    // Rose Pine Dawn's warm surface, for cards on the background without hard-coding a theme.
    let background = c("background")?;
    let foreground = c("foreground").unwrap_or_else(|| "#575279".into());
    let dark_background = c("dark_background").unwrap_or_else(|| background.clone());
    let dark_foreground = c("dark_foreground").unwrap_or_else(|| foreground.clone());
    let selection = c("selection").unwrap_or_else(|| dark_background.clone());
    let muted = c("muted").unwrap_or_else(|| selection.clone());
    let accent = c("accent").unwrap_or_else(|| selection.clone());
    let red = c("red").unwrap_or_else(|| "#b4637a".into());
    let card = mix(&background, "#fffaf3", 0.55);
    let palette = serde_json::json!({
        "background": background, "foreground": foreground,
        "card": card, "card-foreground": foreground,
        "popover": card, "popover-foreground": foreground,
        "primary": accent, "primary-foreground": background,
        "secondary": selection, "secondary-foreground": foreground,
        "muted": dark_background, "muted-foreground": dark_foreground,
        "accent": selection, "accent-foreground": foreground,
        "destructive": red, "border": muted, "input": muted, "ring": accent,
        "chart-1": c("blue").unwrap_or_else(|| accent.clone()),
        "chart-2": c("yellow").unwrap_or_else(|| red.clone()),
        "chart-3": c("green").unwrap_or_else(|| accent.clone()),
        "chart-4": c("magenta").unwrap_or_else(|| foreground.clone()),
        "chart-5": red,
    });
    Some(serde_json::json!({
        "cssVars": { "theme": { "radius": "10px" }, "light": palette, "dark": palette }
    }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROSE_PINE_DAWN: &str = "\u{feff}mode = \"light\"\naccent = \"#56949f\"\nselection = \"#dfdad9\"\n\
        muted = \"#cecacd\"\nbackground = \"#faf4ed\"\ndark_background = \"#ede7e1\"\n\
        foreground = \"#575279\"\ndark_foreground = \"#9893a5\"\nred = \"#b4637a\"\ngreen = \"#286983\"\n";

    #[test]
    fn parses_the_palette_into_css_vars() {
        let json = overrides_from_colors(ROSE_PINE_DAWN).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let vars = &value["cssVars"]["light"];
        assert_eq!(vars["background"], "#faf4ed");
        assert_eq!(vars["primary"], "#56949f");
        assert_eq!(vars["border"], "#cecacd");
        assert_eq!(vars["muted-foreground"], "#9893a5");
        // The same palette serves the dark appearance.
        assert_eq!(value["cssVars"]["dark"], *vars);
        // Card is a mix of the background and the warm surface, still warm.
        assert!(vars["card"].as_str().unwrap().starts_with("#f"));
        assert_eq!(value["cssVars"]["theme"]["radius"], "10px");
    }

    #[test]
    fn a_palette_without_background_is_rejected() {
        assert!(overrides_from_colors("mode = \"light\"\n").is_none());
        assert!(overrides_from_colors("").is_none());
    }

    #[test]
    fn non_color_lines_are_skipped() {
        let colors = parse_colors("mode = \"light\"\naccent = \"#56949f\"\nbroken = \"#12345\"\n");
        assert_eq!(colors.get("accent").map(String::as_str), Some("#56949f"));
        assert_eq!(colors.len(), 1);
    }

    #[test]
    fn mix_moves_toward_the_target() {
        assert_eq!(mix("#000000", "#ffffff", 0.5), "#808080");
        assert_eq!(mix("#faf4ed", "#faf4ed", 0.5), "#faf4ed");
    }
}
