//! The active Omarchy theme as nsh theme overrides. Omarchy materializes the current theme's
//! palette at `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` and does not theme GTK apps,
//! so this window reads the palette itself and maps it onto nsh's shadcn tokens. One palette
//! serves both appearances (an Omarchy theme already matches its own `mode`); tokens left out
//! keep nsh's defaults. Colors are precomputed hex (no `var()`/`color-mix()` in the output).
//! `watch_live` re-applies the palette when Omarchy switches themes (`omarchy theme set`
//! rewrites the materialized files and bumps `theme.generation`), the way native apps follow
//! the system theme — nsh re-resolves token classes at once, so no window rebuild.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use nsh_platform_linux as platform;

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

/// Cheap change signature: the `omarchy theme set` generation counter plus the palette's
/// mtime (covers manual palette edits); `None` when no Omarchy state exists.
fn theme_signature() -> Option<(Option<String>, Option<std::time::SystemTime>)> {
    let colors = colors_toml()?;
    // theme.generation sits beside the theme directory: <state>/omarchy/current/theme.generation.
    let generation = std::fs::read_to_string(colors.parent()?.parent()?.join("theme.generation")).ok();
    let mtime = std::fs::metadata(&colors).ok().and_then(|m| m.modified().ok());
    Some((generation, mtime))
}

/// Re-applies the Omarchy palette whenever it changes, on the UI thread's main loop.
/// Installed once after the initial palette; polling a stat is free at this rate.
pub fn watch_live(view: Rc<platform::AppView>) {
    let mut last = theme_signature();
    platform::gtk4::glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        let signature = theme_signature();
        if signature == last { return platform::gtk4::glib::ControlFlow::Continue; }
        last = signature;
        match omarchy_overrides() {
            Some(json) => if let Err(error) = view.set_theme_overrides(Some(&json)) {
                log::warn!("Omarchy theme update rejected: {error}");
            },
            // The palette vanished (no Omarchy state): hand the app its own theme back.
            None => { let _ = view.set_theme_overrides(None); }
        }
        platform::gtk4::glib::ControlFlow::Continue
    });
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


/// The palette of `colors.toml` text as shadcn cssVars. `None` when the file lacks
/// `background` or `foreground` (not a usable palette; the caller falls back to the default).
/// Every token is derived from the theme's own keys — nothing is hard-coded, so dark themes
/// (Catppuccin: `lighter_background` is the raised card surface) and light ones (Rose Pine:
/// it is the official surface layer) both map correctly. Tokens the palette does not define
/// (chart colors) are omitted: nsh keeps its own defaults rather than a wrong color.
fn overrides_from_colors(text: &str) -> Option<String> {
    let colors = parse_colors(text);
    let c = |key: &str| colors.get(key).cloned();
    let background = c("background")?;
    let foreground = c("foreground")?;
    // Omarchy key semantics: dark_background is one step deeper than the background (muted
    // panels), lighter_background one step up (the card surface), dark_foreground is the
    // dimmed secondary text — independent of the theme's own light/dark mode.
    let muted = c("dark_background").unwrap_or_else(|| background.clone());
    let card = c("lighter_background").unwrap_or_else(|| muted.clone());
    let selection = c("selection").unwrap_or_else(|| muted.clone());
    let accent = c("accent").unwrap_or_else(|| selection.clone());
    // ... and the palette's own `muted` is its outline tone (borders, inputs).
    let dimmed = c("dark_foreground").unwrap_or_else(|| foreground.clone());
    let outline = c("muted").unwrap_or_else(|| muted.clone());
    let mut palette = serde_json::Map::new();
    let mut put = |k: &str, v: String| { palette.insert(k.into(), serde_json::Value::String(v)); };
    put("background", background.clone());
    put("foreground", foreground.clone());
    put("card", card.clone());
    put("card-foreground", foreground.clone());
    put("popover", card);
    put("popover-foreground", foreground.clone());
    put("primary", accent.clone());
    put("primary-foreground", background);
    put("secondary", selection.clone());
    put("secondary-foreground", foreground.clone());
    put("muted", muted);
    put("muted-foreground", dimmed);
    put("accent", selection);
    put("accent-foreground", foreground);
    put("border", outline.clone());
    put("input", outline);
    put("ring", accent);
    if let Some(red) = c("red") { put("destructive", red.clone()); put("chart-5", red); }
    for (token, key) in [("chart-1", "blue"), ("chart-2", "yellow"), ("chart-3", "green"), ("chart-4", "magenta")] {
        if let Some(color) = c(key) { put(token, color); }
    }
    let dark = serde_json::Value::Object(palette.clone());
    let light = serde_json::Value::Object(palette);
    Some(serde_json::json!({
        "cssVars": { "theme": { "radius": "10px" }, "light": light, "dark": dark }
    }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROSE_PINE_DAWN: &str = "\u{feff}mode = \"light\"\naccent = \"#56949f\"\nselection = \"#dfdad9\"\n\
        muted = \"#cecacd\"\nbackground = \"#faf4ed\"\ndark_background = \"#ede7e1\"\nlighter_background = \"#f2e9e1\"\n\
        foreground = \"#575279\"\ndark_foreground = \"#9893a5\"\nred = \"#b4637a\"\ngreen = \"#286983\"\n";

    #[test]
    fn parses_the_palette_into_css_vars() {
        let json = overrides_from_colors(ROSE_PINE_DAWN).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let vars = &value["cssVars"]["light"];
        assert_eq!(vars["background"], "#faf4ed");
        assert_eq!(vars["primary"], "#56949f");
        assert_eq!(vars["primary-foreground"], "#faf4ed");
        // The card is the theme's own surface layer, never a mix toward a fixed color.
        assert_eq!(vars["card"], "#f2e9e1");
        assert_eq!(vars["popover"], "#f2e9e1");
        assert_eq!(vars["muted"], "#ede7e1");
        assert_eq!(vars["muted-foreground"], "#9893a5");
        assert_eq!(vars["border"], "#cecacd");
        assert_eq!(vars["input"], "#cecacd");
        assert_eq!(vars["destructive"], "#b4637a");
        // Chart colors come from the palette's own keys; missing ones keep nsh defaults.
        assert_eq!(vars["chart-3"], "#286983");
        assert_eq!(vars["chart-5"], "#b4637a");
        assert!(vars["chart-1"].is_null());
        // The same palette serves the dark appearance.
        assert_eq!(value["cssVars"]["dark"], *vars);
        assert_eq!(value["cssVars"]["theme"]["radius"], "10px");
    }

    #[test]
    fn dark_themes_keep_dark_cards() {
        // Catppuccin: surface0 (lighter_background) is the raised card on its dark
        // background — nothing may lighten it back toward white.
        let json = overrides_from_colors(
            "background = \"#1e1e2e\"\nforeground = \"#cdd6f4\"\naccent = \"#89b4fa\"\n\
             selection = \"#45475a\"\nmuted = \"#585b70\"\ndark_background = \"#161622\"\n\
             lighter_background = \"#313244\"\ndark_foreground = \"#6c7086\"\nred = \"#f38ba8\"\n",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let vars = &value["cssVars"]["dark"];
        assert_eq!(vars["background"], "#1e1e2e");
        assert_eq!(vars["card"], "#313244");
        assert_eq!(vars["primary"], "#89b4fa");
        assert_eq!(vars["primary-foreground"], "#1e1e2e");
        assert_eq!(vars["border"], "#585b70");
        assert_eq!(vars["muted-foreground"], "#6c7086");
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
}
