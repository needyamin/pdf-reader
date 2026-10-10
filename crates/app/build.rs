//! Windows resource compilation.
//!
//! Embeds `assets/icon.ico` into `pdf-reader.exe` so Explorer, the taskbar and
//! Alt-Tab show the application icon instead of the generic OS one. The icon is
//! only half of the story — the running window also needs it at runtime, which
//! `main.rs` does with `ViewportBuilder::with_icon` — but without a resource the
//! file itself stays unbranded in Explorer.
//!
//! It also writes a `VERSIONINFO` resource, so right-click → Properties → Details
//! shows the version. The numbers come from `CARGO_PKG_VERSION`, the same
//! workspace value the About box, the NSIS uninstall entry and the MSIX manifest
//! read, so the four cannot disagree about what was shipped. Without it the file
//! carried no version at all, and Explorer's Details tab was blank.
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
    if let Err(error) = windows_resources() {
        println!("cargo:warning={error}");
    }
}

/// The version as the four numbers a `VERSIONINFO` resource requires.
///
/// `FILEVERSION` is `a,b,c,d`, so a short version is padded and any pre-release
/// suffix is dropped: `5.0.0` becomes `5,0,0,0`, and `5.1.0-beta.2` also becomes
/// `5,1,0,0`.
#[cfg(windows)]
fn version_quad(version: &str) -> String {
    let mut parts: Vec<u32> = version
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|digit| digit.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    parts.resize(4, 0);
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Quote a string for a `.rc` file.
///
/// The resource compiler treats a backslash inside a quoted string as an escape,
/// so both it and the quote character have to be escaped.
#[cfg(windows)]
fn rc_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Compile the icon and the version block into a Windows resource.
#[cfg(windows)]
fn windows_resources() -> Result<(), String> {
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
    let version = env!("CARGO_PKG_VERSION");
    let quad = version_quad(version);
    let description = rc_string(env!("CARGO_PKG_DESCRIPTION"));
    let authors = rc_string(&env!("CARGO_PKG_AUTHORS").replace(',', ", "));
    let license = rc_string(env!("CARGO_PKG_LICENSE"));

    let rc = format!(
        r#"IDI_ICON1 ICON DISCARDABLE "{icon_path}"

1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "{authors}"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "pdf-reader"
      VALUE "LegalCopyright", "{license}"
      VALUE "OriginalFilename", "pdf-reader.exe"
      VALUE "ProductName", "{description}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    std::fs::write(&rc_path, rc)
        .map_err(|error| format!("cannot write {}: {error}", rc_path.display()))?;

    // `manifest_optional`: a missing resource compiler is a cosmetic loss, not
    // a reason to break the build. `NONE` = no preprocessor macros, no includes.
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .map_err(|result| result.to_string())
}

// No unit tests here on purpose: a build script is compiled and run as a plain
// program, so `#[cfg(test)]` inside it is never exercised by `cargo test` and
// tests written here would be coverage that never runs. The rule these two
// helpers implement is checked against the built artifact instead — right-click
// the release exe → Properties → Details reads `5.0.0` — and a version string
// they cannot render makes the resource compiler reject the block, which
// surfaces as a `cargo:warning` rather than silently shipping an unversioned
// file.
