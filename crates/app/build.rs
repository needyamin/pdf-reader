//! Windows resource compilation.
//!
//! Embeds `assets/icon.ico` into `pdf-reader.exe` so Explorer, the taskbar and
//! Alt-Tab show the application icon instead of the generic OS one. The icon is
//! only half of the story — the running window also needs it at runtime, which
//! `main.rs` does with `ViewportBuilder::with_icon` — but without a resource the
//! file itself stays unbranded in Explorer.
//!
//! Nothing is compiled on other platforms: macOS takes its icon from the app
//! bundle and Linux from a `.desktop` file, so an embedded resource is a no-op
//! there.

fn main() {
    // Rebuild when either asset changes. Without this the resource would keep
    // pointing at whatever the icon looked like at the last clean build.
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=../../assets/icon.png");

    #[cfg(windows)]
    if let Err(error) = windows_icon() {
        println!("cargo:warning={error}");
    }
}

/// Compile `assets/icon.ico` into a Windows resource and link it into the binary.
#[cfg(windows)]
fn windows_icon() -> Result<(), String> {
    use std::path::PathBuf;

    let icon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon.ico");
    let icon = icon
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", icon.display()))?;

    let out_dir = PathBuf::from(
        std::env::var("OUT_DIR").map_err(|error| format!("OUT_DIR not set: {error}"))?,
    );
    let rc_path = out_dir.join("app_icon.rc");

    // An absolute path is written out rather than a relative one because the
    // resource compiler's working directory is not guaranteed to be the crate
    // root. Forward slashes: rc.exe treats a backslash inside a quoted path as
    // an escape.
    let icon_path = icon.display().to_string().replace('\\', "/");
    std::fs::write(
        &rc_path,
        format!("IDI_ICON1 ICON DISCARDABLE \"{icon_path}\"\n"),
    )
    .map_err(|error| format!("cannot write {}: {error}", rc_path.display()))?;

    // `manifest_optional`: a missing resource compiler is a cosmetic loss, not
    // a reason to break the build. `NONE` = no preprocessor macros, no includes.
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .map_err(|result| result.to_string())
}
