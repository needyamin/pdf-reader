//! Windows packaging targets: portable zip, NSIS setup, Inno Setup and MSIX.
//!
//! Every target shares one flow: stage `PDFium`, build the release binary, copy
//! the ship set (`pdf-reader.exe`, `pdfium.dll`, `LICENSE`, `PDFium`'s
//! third-party licence texts) into `target/dist/stage`, then hand that
//! directory to the packager. The installer definitions (`.nsi`, `.iss`,
//! `AppxManifest.xml`) are generated into `target/dist` at run time so they can
//! never drift from the version in the workspace manifest.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{binary_name, cargo, fetch_pdfium, host_triple, staged_dir};

/// Product name shared by every package format.
const PRODUCT: &str = "PDF Reader";
/// Publisher shared by every package format.
const PUBLISHER: &str = "YAMiN HOSSAIN";
/// MSIX identity name; per the Appx schema it must not contain spaces.
const MSIX_IDENTITY: &str = "YAMinHossain.PDFReader";
/// MSIX publisher; must match the signing certificate subject exactly.
const MSIX_PUBLISHER: &str = "CN=YAMiN HOSSAIN";
/// Subject name `signtool` matches against (the bare CN, no prefix).
const MSIX_SIGN_SUBJECT: &str = "YAMiN HOSSAIN";
/// Where every artifact lands.
const DIST: &str = "target/dist";

/// PNG asset names and sizes `MakeAppx` expects for the manifest references.
const MSIX_ASSETS: &[(&str, u32)] = &[
    ("Square44x44Logo.png", 44),
    ("Square44x44Logo.targetsize-16.png", 16),
    ("Square44x44Logo.targetsize-24.png", 24),
    ("Square44x44Logo.targetsize-32.png", 32),
    ("Square44x44Logo.targetsize-48.png", 48),
    ("Square44x44Logo.targetsize-256.png", 256),
    ("Square150x150Logo.png", 150),
    ("StoreLogo.png", 50),
];

/// One release build plus the staged ship set shared by all packagers.
struct Stage {
    /// Workspace version, e.g. `0.1.0`.
    version: String,
    /// Directory holding `pdf-reader.exe`, `pdfium.dll`, `LICENSE`, `licenses/`.
    dir: PathBuf,
}

/// Build `pdf-reader-<version>-portable-x64.zip`.
///
/// The zip contains one top-level `pdf-reader-<version>` folder; the app finds
/// `pdfium.dll` beside itself, so it runs from wherever the folder is unpacked.
pub fn portable() -> Result<PathBuf> {
    let stage = stage()?;
    let root = dist_root();

    let name = format!("pdf-reader-{}", stage.version);
    let folder = root.join("portable").join(&name);
    if folder.exists() {
        fs::remove_dir_all(&folder).with_context(|| format!("clearing {}", folder.display()))?;
    }
    fs::create_dir_all(&folder)?;
    copy_stage(&stage.dir, &folder)?;

    let zip = std::path::absolute(root.join(format!("{name}-portable-x64.zip")))?;
    let parent = folder
        .parent()
        .context("portable folder has no parent")?
        .to_path_buf();

    // bsdtar ships with Windows; `-a` picks zip from the file extension.
    let status = Command::new("tar")
        .current_dir(&parent)
        .args(["-a", "-c", "-f"])
        .arg(&zip)
        .arg(&name)
        .status()
        .context("running tar (bsdtar ships with Windows)")?;
    if !status.success() {
        bail!("tar failed");
    }

    println!("portable package: {}", zip.display());
    Ok(zip)
}

/// Build `pdf-reader-<version>-setup.exe` with NSIS.
pub fn nsis() -> Result<PathBuf> {
    let stage = stage()?;
    let root = std::path::absolute(dist_root())?;
    let stage_dir = std::path::absolute(&stage.dir)?;
    let icon = std::path::absolute("assets/icon.ico")?;
    let script = root.join("pdf-reader.nsi");
    let out = root.join(format!("pdf-reader-{}-setup.exe", stage.version));

    let text = render(
        NSIS_TEMPLATE,
        &[
            ("PRODUCT", PRODUCT.to_string()),
            ("PUBLISHER", PUBLISHER.to_string()),
            ("VERSION", stage.version.clone()),
            ("STAGE", stage_dir.display().to_string()),
            ("ICON", icon.display().to_string()),
            ("LICENSE", stage_dir.join("LICENSE").display().to_string()),
            ("OUT", out.display().to_string()),
        ],
    );
    fs::write(&script, text).with_context(|| format!("writing {}", script.display()))?;

    let makensis = find_makensis()?;
    let status = Command::new(&makensis)
        .arg("/V2")
        .arg(&script)
        .status()
        .with_context(|| format!("running {}", makensis.display()))?;
    if !status.success() {
        bail!("makensis failed");
    }

    if !out.is_file() {
        bail!("makensis did not produce {}", out.display());
    }
    println!("NSIS installer: {}", out.display());
    Ok(out)
}

/// Build `pdf-reader-<version>-inno-setup.exe` with Inno Setup.
pub fn inno() -> Result<PathBuf> {
    let stage = stage()?;
    let root = std::path::absolute(dist_root())?;
    let stage_dir = std::path::absolute(&stage.dir)?;
    let icon = std::path::absolute("assets/icon.ico")?;
    let script = root.join("pdf-reader.iss");

    let text = render(
        INNO_TEMPLATE,
        &[
            ("PRODUCT", PRODUCT.to_string()),
            ("PUBLISHER", PUBLISHER.to_string()),
            ("VERSION", stage.version.clone()),
            ("STAGE", stage_dir.display().to_string()),
            ("DIST", root.display().to_string()),
            ("ICON", icon.display().to_string()),
            ("LICENSE", stage_dir.join("LICENSE").display().to_string()),
        ],
    );
    fs::write(&script, text).with_context(|| format!("writing {}", script.display()))?;

    let iscc = find_iscc()?;
    let status = Command::new(&iscc)
        .arg(&script)
        .status()
        .with_context(|| format!("running {}", iscc.display()))?;
    if !status.success() {
        bail!("ISCC failed");
    }

    let out = root.join(format!("pdf-reader-{}-inno-setup.exe", stage.version));
    if !out.is_file() {
        bail!("ISCC did not produce {}", out.display());
    }
    println!("Inno Setup installer: {}", out.display());
    Ok(out)
}

/// Build `pdf-reader-<version>-x64.msix` with `MakeAppx`, signing it by default.
///
/// A classic Win32 desktop app ships as a full-trust MSIX. Signing uses a
/// self-signed code-signing certificate (`CN=YAMiN HOSSAIN`) created in the
/// current-user store and reused across runs; `signtool` signs straight from
/// the store, so the private key never leaves it. Windows only installs the
/// package after that certificate is trusted, so the task prints the
/// `certutil` command for it. `--no-sign` skips signing entirely.
pub fn msix(no_sign: bool) -> Result<PathBuf> {
    let stage = stage()?;
    let root = std::path::absolute(dist_root())?;

    let package = root.join("msix").join("package");
    if package.exists() {
        fs::remove_dir_all(&package).with_context(|| format!("clearing {}", package.display()))?;
    }
    let assets = package.join("Assets");
    fs::create_dir_all(&assets)?;
    copy_stage(&stage.dir, &package)?;
    write_msix_assets(&assets)?;

    let manifest = package.join("AppxManifest.xml");
    let text = render(
        MSIX_MANIFEST,
        &[
            ("PRODUCT", PRODUCT.to_string()),
            ("PUBLISHER", PUBLISHER.to_string()),
            ("MSIX_IDENTITY", MSIX_IDENTITY.to_string()),
            ("MSIX_PUBLISHER", MSIX_PUBLISHER.to_string()),
            ("MSIX_VERSION", msix_version(&stage.version)),
        ],
    );
    fs::write(&manifest, text).with_context(|| format!("writing {}", manifest.display()))?;

    let out = std::path::absolute(root.join(format!(
        "pdf-reader-{}-x64.msix",
        stage.version
    )))?;
    if out.is_file() {
        fs::remove_file(&out)?;
    }

    let makeappx = find_sdk_tool("makeappx.exe")?;
    let status = Command::new(&makeappx)
        .args(["pack", "/o", "/d"])
        .arg(&package)
        .args(["/p"])
        .arg(&out)
        .status()
        .with_context(|| format!("running {}", makeappx.display()))?;
    if !status.success() {
        bail!("makeappx pack failed");
    }

    if no_sign {
        println!("MSIX package (unsigned): {}", out.display());
        return Ok(out);
    }

    let cer = signing_certificate(&root)?;
    let signtool = find_sdk_tool("signtool.exe")?;
    let status = Command::new(&signtool)
        .args(["sign", "/fd", "SHA256", "/n", MSIX_SIGN_SUBJECT])
        .arg(&out)
        .status()
        .with_context(|| format!("running {}", signtool.display()))?;
    if !status.success() {
        bail!("signtool failed (use `cargo xtask dist:msix -- --no-sign` to skip signing)");
    }

    println!("MSIX package: {}", out.display());
    println!("A self-signed package only installs once its certificate is trusted:");
    println!("  certutil -user -addstore Root \"{}\"", cer.display());
    Ok(out)
}

/// Ensure a Windows host, then build and stage everything the packagers need.
fn stage() -> Result<Stage> {
    if !cfg!(windows) {
        bail!("the dist:* packaging tasks target Windows and must run on Windows");
    }

    let triple = host_triple()?;
    fetch_pdfium(&triple, false)?;
    cargo(&["build", "--release", "-p", "pdfreader-app"])?;

    let version = workspace_version()?;
    let exe = PathBuf::from("target/release").join(binary_name());
    if !exe.is_file() {
        bail!("release build did not produce {}", exe.display());
    }

    let dir = PathBuf::from(DIST).join("stage");
    fs::create_dir_all(&dir)?;

    fs::copy(&exe, dir.join(binary_name()))
        .with_context(|| format!("copying {}", exe.display()))?;
    let dll = staged_dir(&triple).join("pdfium.dll");
    fs::copy(&dll, dir.join("pdfium.dll"))
        .with_context(|| format!("copying {}", dll.display()))?;
    fs::copy("LICENSE", dir.join("LICENSE")).context("copying LICENSE")?;

    // PDFium's third-party licence texts ship beside the app so binary
    // redistribution keeps the attributions.
    let licenses = dir.join("licenses");
    fs::create_dir_all(&licenses)?;
    let source = staged_dir(&triple).join("licenses");
    if source.is_dir() {
        for file in fs::read_dir(&source).context("listing PDFium licences")? {
            let file = file?;
            if file.file_type()?.is_file() {
                fs::copy(file.path(), licenses.join(file.file_name()))?;
            }
        }
    }

    Ok(Stage { version, dir })
}

/// Copy the staged ship set (files plus the `licenses/` directory) to `dest`.
fn copy_stage(stage: &Path, dest: &Path) -> Result<()> {
    for entry in fs::read_dir(stage).context("reading stage directory")? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            for file in fs::read_dir(entry.path())? {
                let file = file?;
                if file.file_type()?.is_file() {
                    fs::copy(file.path(), target.join(file.file_name()))?;
                }
            }
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The workspace version from `[workspace.package]`, the single source of truth.
fn workspace_version() -> Result<String> {
    let text = fs::read_to_string("Cargo.toml").context("reading workspace Cargo.toml")?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[workspace.package]";
            continue;
        }
        if in_package
            && let Some(rest) = trimmed.strip_prefix("version")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Ok(value.trim().trim_matches('"').to_string());
        }
    }
    bail!("workspace Cargo.toml has no [workspace.package] version")
}

/// An MSIX `Version` is exactly four dot-separated integers.
fn msix_version(version: &str) -> String {
    if version.split('.').count() == 3 {
        format!("{version}.0")
    } else {
        version.to_string()
    }
}

/// Replace `@KEY@` placeholders in a template.
///
/// The templates are full of `${...}` (NSIS), `""` (Inno) and XML, so token
/// replacement beats `format!` escaping.
fn render(template: &str, pairs: &[(&str, String)]) -> String {
    let mut text = template.to_string();
    for (key, value) in pairs {
        text = text.replace(&format!("@{key}@"), value);
    }
    text
}

/// `target/dist`, the root of every artifact.
fn dist_root() -> PathBuf {
    PathBuf::from(DIST)
}

/// Find `name` on the process PATH.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The per-user local application-data directory.
///
/// `LOCALAPPDATA` is not always set (some service and CI shells run without
/// it), so fall back to `%USERPROFILE%\AppData\Local`.
fn local_app_data() -> Option<PathBuf> {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let path = PathBuf::from(local);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    std::env::var_os("USERPROFILE")
        .map(|profile| PathBuf::from(profile).join("AppData").join("Local"))
        .filter(|path| !path.as_os_str().is_empty())
}

/// Locate `makensis.exe`: PATH first, then the machine and per-user NSIS
/// install directories.
fn find_makensis() -> Result<PathBuf> {
    if let Some(path) = find_on_path("makensis.exe") {
        return Ok(path);
    }
    let mut dirs = vec![
        "C:/Program Files (x86)/NSIS".to_string(),
        "C:/Program Files/NSIS".to_string(),
    ];
    if let Some(local) = local_app_data() {
        dirs.push(
            local
                .join("Programs")
                .join("NSIS")
                .display()
                .to_string(),
        );
    }
    for dir in dirs {
        let candidate = PathBuf::from(&dir).join("makensis.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("makensis.exe not found.\nInstall NSIS with:\n  winget install -e --id NSIS.NSIS")
}

/// Locate `ISCC.exe`: PATH first, then the machine and per-user Inno Setup 6
/// install directories.
fn find_iscc() -> Result<PathBuf> {
    if let Some(path) = find_on_path("ISCC.exe") {
        return Ok(path);
    }
    let mut dirs = vec![
        "C:/Program Files (x86)/Inno Setup 6".to_string(),
        "C:/Program Files/Inno Setup 6".to_string(),
    ];
    if let Some(local) = local_app_data() {
        dirs.push(
            local
                .join("Programs")
                .join("Inno Setup 6")
                .display()
                .to_string(),
        );
    }
    for dir in dirs {
        let candidate = PathBuf::from(&dir).join("ISCC.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("ISCC.exe not found.\nInstall Inno Setup with:\n  winget install -e --id JRSoftware.InnoSetup")
}

/// Locate a Windows SDK tool (makeappx, signtool) across installed kit versions.
fn find_sdk_tool(name: &str) -> Result<PathBuf> {
    if let Some(path) = find_on_path(name) {
        return Ok(path);
    }

    let root = PathBuf::from("C:/Program Files (x86)/Windows Kits/10/bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(&root)
        .with_context(|| format!("reading {}", root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("10."))
        })
        .collect();
    versions.sort();

    for version in versions.iter().rev() {
        for arch in ["x64", "x86"] {
            let candidate = version.join(arch).join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    bail!("Windows SDK tool {name} not found.\nInstall the Windows SDK with:\n  winget install -e --id Microsoft.WindowsSDK.10.0.26100")
}

/// Create (or reuse) the self-signed code-signing certificate and export the
/// public `.cer` for the trust hint.
///
/// The certificate lives in `Cert:\CurrentUser\My` under the MSIX publisher
/// subject and is reused across runs; the private key never leaves the store,
/// because `signtool` signs from there by subject name.
fn signing_certificate(root: &Path) -> Result<PathBuf> {
    let cer = root.join("pdf-reader-sign.cer");
    let script = root.join("make-signing-cert.ps1");
    let text = render(SIGN_CERT_SCRIPT, &[("MSIX_PUBLISHER", MSIX_PUBLISHER.to_string())]);
    fs::write(&script, text).with_context(|| format!("writing {}", script.display()))?;

    let status = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(&cer)
        .status()
        .context("running powershell for the signing certificate")?;
    if !status.success() {
        bail!("creating the self-signed signing certificate failed");
    }
    if !cer.is_file() {
        bail!("signing certificate export was not produced");
    }

    Ok(cer)
}

/// Scale `assets/icon.png` to every PNG asset the MSIX manifest references.
fn write_msix_assets(assets: &Path) -> Result<()> {
    let icon = image::open("assets/icon.png").context("decoding assets/icon.png")?;
    for (name, size) in MSIX_ASSETS {
        let scaled = icon.resize_exact(*size, *size, image::imageops::FilterType::Lanczos3);
        scaled
            .save_with_format(assets.join(name), image::ImageFormat::Png)
            .with_context(|| format!("writing {name}"))?;
    }
    Ok(())
}


/// NSIS script for the setup installer.
const NSIS_TEMPLATE: &str = r#"
; Generated by `cargo xtask dist:nsis` - edit the generator in xtask/src/dist.rs.
Unicode true
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!define PRODUCT "@PRODUCT@"
!define PUBLISHER "@PUBLISHER@"
!define VERSION "@VERSION@"
!define EXE "pdf-reader.exe"

!include "MUI2.nsh"
!define MUI_ICON "@ICON@"
!define MUI_UNICON "@ICON@"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "@LICENSE@"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Name "${PRODUCT} ${VERSION}"
OutFile "@OUT@"
InstallDir "$PROGRAMFILES64\${PRODUCT}"

Section "Install"
  SetOutPath "$INSTDIR"
  File "@STAGE@\pdf-reader.exe"
  File "@STAGE@\pdfium.dll"
  File "@STAGE@\LICENSE"
  SetOutPath "$INSTDIR\licenses"
  File "@STAGE@\licenses\*.*"

  SetOutPath "$INSTDIR"
  CreateDirectory "$SMPROGRAMS\${PRODUCT}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT}\${PRODUCT}.lnk" "$INSTDIR\${EXE}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT}\Uninstall ${PRODUCT}.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}" "NoRepair" 1

  ; Advertise .pdf in "Open with" and Default Programs without stealing the
  ; user's current default viewer.
  WriteRegStr HKLM "Software\Classes\PDFReader.PDF" "" "PDF Document"
  WriteRegStr HKLM "Software\Classes\PDFReader.PDF\DefaultIcon" "" "$INSTDIR\${EXE},0"
  WriteRegStr HKLM "Software\Classes\PDFReader.PDF\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  WriteRegStr HKLM "Software\Classes\Applications\${EXE}\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  WriteRegStr HKLM "Software\Classes\Applications\${EXE}\SupportedTypes" ".pdf" ""
  WriteRegStr HKLM "Software\${PRODUCT}\Capabilities" "ApplicationName" "${PRODUCT}"
  WriteRegStr HKLM "Software\${PRODUCT}\Capabilities" "ApplicationDescription" "A fast, native PDF reader."
  WriteRegStr HKLM "Software\${PRODUCT}\Capabilities\FileAssociations" ".pdf" "PDFReader.PDF"
  WriteRegStr HKLM "Software\RegisteredApplications" "${PRODUCT}" "Software\${PRODUCT}\Capabilities"

  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  RMDir /r "$INSTDIR"
  RMDir /r "$SMPROGRAMS\${PRODUCT}"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"
  DeleteRegKey HKLM "Software\Classes\PDFReader.PDF"
  DeleteRegKey HKLM "Software\Classes\Applications\${EXE}"
  DeleteRegValue HKLM "Software\RegisteredApplications" "${PRODUCT}"
  DeleteRegKey /ifempty HKLM "Software\${PRODUCT}\Capabilities\FileAssociations"
  DeleteRegKey /ifempty HKLM "Software\${PRODUCT}\Capabilities"
  DeleteRegKey /ifempty HKLM "Software\${PRODUCT}"
SectionEnd
"#;

/// Inno Setup script for the second installer flavour.
const INNO_TEMPLATE: &str = r#"
; Generated by `cargo xtask dist:inno` - edit the generator in xtask/src/dist.rs.

[Setup]
; Fixed AppId so a newer setup upgrades in place instead of stacking installs.
AppId={{7E1B2F4A-9C3D-4E5F-A6B7-8C9D0E1F2A3B}}
AppName=@PRODUCT@
AppVersion=@VERSION@
AppPublisher=@PUBLISHER@
DefaultDirName={autopf}\@PRODUCT@
DefaultGroupName=@PRODUCT@
DisableProgramGroupPage=yes
OutputDir=@DIST@
OutputBaseFilename=pdf-reader-@VERSION@-inno-setup
SetupIconFile=@ICON@
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=@LICENSE@

[Files]
Source: "@STAGE@\pdf-reader.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "@STAGE@\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "@STAGE@\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "@STAGE@\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{group}\@PRODUCT@"; Filename: "{app}\pdf-reader.exe"
Name: "{group}\Uninstall @PRODUCT@"; Filename: "{uninstallexe}"

[Registry]
; Advertise .pdf in "Open with" and Default Programs without stealing the
; user's current default viewer.
Root: HKLM; Subkey: "Software\Classes\PDFReader.PDF"; ValueType: string; ValueData: "PDF Document"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\PDFReader.PDF\DefaultIcon"; ValueType: string; ValueData: "{app}\pdf-reader.exe,0"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\PDFReader.PDF\shell\open\command"; ValueType: string; ValueData: """{app}\pdf-reader.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Applications\pdf-reader.exe\shell\open\command"; ValueType: string; ValueData: """{app}\pdf-reader.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Applications\pdf-reader.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\@PRODUCT@\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "@PRODUCT@"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\@PRODUCT@\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "A fast, native PDF reader."
Root: HKLM; Subkey: "Software\@PRODUCT@\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "PDFReader.PDF"
Root: HKLM; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "@PRODUCT@"; ValueData: "Software\@PRODUCT@\Capabilities"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\pdf-reader.exe"; Description: "Launch @PRODUCT@"; Flags: nowait postinstall skipifsilent
"#;

/// Appx manifest for the full-trust desktop MSIX package.
const MSIX_MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by `cargo xtask dist:msix` - edit the generator in xtask/src/dist.rs. -->
<Package
    xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
    xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
    xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities">

  <Identity Name="@MSIX_IDENTITY@"
            Version="@MSIX_VERSION@"
            Publisher="@MSIX_PUBLISHER@"
            ProcessorArchitecture="x64" />

  <Properties>
    <DisplayName>@PRODUCT@</DisplayName>
    <PublisherDisplayName>@PUBLISHER@</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>

  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>

  <Resources>
    <Resource Language="en-us" />
  </Resources>

  <Applications>
    <Application Id="App"
                 Executable="pdf-reader.exe"
                 EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements
          DisplayName="@PRODUCT@"
          Description="A fast, native PDF reader."
          BackgroundColor="transparent"
          Square150x150Logo="Assets\Square150x150Logo.png"
          Square44x44Logo="Assets\Square44x44Logo.png">
      </uap:VisualElements>
      <Extensions>
        <uap:Extension Category="windows.fileTypeAssociation">
          <uap:FileTypeAssociation Name="pdfdocument">
            <uap:SupportedFileTypes>
              <uap:FileType>.pdf</uap:FileType>
            </uap:SupportedFileTypes>
          </uap:FileTypeAssociation>
        </uap:Extension>
      </Extensions>
    </Application>
  </Applications>

  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"#;

/// PowerShell that creates or reuses the self-signed signing certificate and
/// exports only the public certificate for the trust hint.
const SIGN_CERT_SCRIPT: &str = r#"
# Generated by `cargo xtask dist:msix` - edit the generator in xtask/src/dist.rs.
param(
    [Parameter(Mandatory = $true)][string]$CerPath
)
$ErrorActionPreference = 'Stop'
$subject = '@MSIX_PUBLISHER@'
$cert = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type Custom -Subject $subject `
        -KeyUsage DigitalSignature -FriendlyName 'PDF Reader MSIX signing' `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    Write-Output "created certificate $($cert.Thumbprint)"
}
else {
    Write-Output "reused certificate $($cert.Thumbprint)"
}
Export-Certificate -Cert $cert -FilePath $CerPath | Out-Null
"#;
