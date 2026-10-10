//! Registering broad-coverage fonts so every script renders.
//!
//! egui ships only a Latin/Cyrillic/Greek subset by default, so Bengali and
//! most other scripts came out as missing-glyph boxes ("tofu") — which read as
//! broken or garbled text and is what made the reader look suspect. We load the
//! Noto fonts dropped into `assets/fonts/` (or `<exe_dir>/fonts` in a packaged
//! build) and register them as fallback faces for the proportional family.
//! egui uses the first face that contains a glyph, so the result is that any
//! bundled script renders, with no change to the UI code that draws text.
//!
//! The loader picks up *every* `.ttf`/`.otf` in those directories, so adding a
//! script later — a CJK font for Chinese/Japanese/Korean, say — is a matter of
//! dropping the file in; no rebuild is required.

use std::path::{Path, PathBuf};

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Register the bundled fonts on a context.
///
/// Call once before the first frame. When no font files are present it leaves
/// egui's defaults in place, so a missing `assets/fonts` directory is
/// harmless rather than fatal.
pub fn install(ctx: &Context) {
    let mut definitions = FontDefinitions::default();
    let mut faces: Vec<String> = Vec::new();

    for dir in candidate_dirs() {
        load_dir(&dir, &mut definitions, &mut faces);
    }

    if faces.is_empty() {
        tracing::warn!("no bundled fonts found in assets/fonts; scripts may render as boxes");
        return; // nothing bundled; keep egui's built-in subset
    }

    // Keep egui's own fallback faces (emoji and so on) after ours.
    if let Some(existing) = definitions.families.get(&FontFamily::Proportional) {
        for name in existing {
            if !faces.contains(name) {
                faces.push(name.clone());
            }
        }
    }
    definitions
        .families
        .insert(FontFamily::Proportional, faces.clone());

    // Expose each script font as its own named family, so the user can pick one
    // for form text (Settings → Text). Every family the UI offers is registered
    // even when its file is missing — otherwise a saved choice would name a
    // family egui does not know and panic. Each named family keeps the full
    // fallback chain, so a glyph the chosen face lacks still renders.
    let mut named: Vec<String> = faces.clone();
    for (name, _) in pdfreader_ui::FORM_FONTS {
        if !name.is_empty() && !named.iter().any(|n| n == name) {
            named.push((*name).to_string());
        }
    }
    for name in &named {
        let mut family: Vec<String> = Vec::new();
        if faces.contains(name) {
            family.push(name.clone());
        }
        for fallback in &faces {
            if !family.contains(fallback) {
                family.push(fallback.clone());
            }
        }
        definitions
            .families
            .insert(FontFamily::Name(name.clone().into()), family);
    }

    tracing::info!("bundled fonts registered: {}", faces.join(", "));
    ctx.set_fonts(definitions);
}

/// Where the font files are looked for, given the executable's folder.
///
/// Beside the executable first (so a packaged install works regardless of the
/// current directory), then `assets/fonts` relative to the working directory,
/// which is how a `cargo run` from the repository root finds them.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(exe_dir) = super::exe_dir() {
        dirs.push(exe_dir.join("fonts"));
    }
    dirs.push(PathBuf::from("assets/fonts"));
    dirs
}

/// Load every `.ttf`/`.otf` in `dir` into `defs`, recording the added face
/// names in `faces`. Missing or unreadable directories are silently skipped.
fn load_dir(dir: &Path, defs: &mut FontDefinitions, faces: &mut Vec<String>) {
    if !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "ttf" && ext != "otf" {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom")
            .to_string();
        if faces.contains(&name) {
            continue;
        }
        defs.font_data
            .insert(name.clone(), FontData::from_owned(bytes).into());
        faces.push(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_dirs_include_exe_and_assets() {
        let dirs = candidate_dirs();
        assert!(dirs.iter().any(|d| d.ends_with("fonts")));
        assert!(dirs.iter().any(|d| d == &PathBuf::from("assets/fonts")));
    }

    #[test]
    fn load_dir_skips_missing_directory_without_panicking() {
        let mut defs = FontDefinitions::default();
        let mut faces = Vec::new();
        // A directory that cannot exist on a sane filesystem.
        load_dir(Path::new("¬/no/such/font/dir"), &mut defs, &mut faces);
        assert!(faces.is_empty());
    }
}
