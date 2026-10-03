//! Build, packaging and release automation.
//!
//! Replaces the previous PowerShell-only scripts with something that runs
//! identically on Windows, macOS and Linux.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

mod dist;

/// Pinned PDFium build. The upstream release is marked immutable, so this tag
/// keeps resolving to the same bytes.
const PDFIUM_TAG: &str = "chromium/8066";
/// Human-readable version matching the pinned tag.
const PDFIUM_VERSION: &str = "156.0.8066.0";

#[derive(Parser)]
#[command(name = "xtask", about = "Build and release automation")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download and stage a pinned PDFium build for a target.
    FetchPdfium {
        /// Rust target triple; defaults to the host.
        #[arg(long)]
        target: Option<String>,
        /// Re-download even if already present.
        #[arg(long)]
        force: bool,
    },
    /// Stage PDFium then build the application.
    Build {
        /// Build in release mode.
        #[arg(long)]
        release: bool,
    },
    /// Stage PDFium then run the application.
    Run {
        /// Build in release mode.
        #[arg(long)]
        release: bool,
        /// Arguments forwarded to the application.
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Print timing and structure for a PDF using the real engine.
    Inspect {
        /// PDF to inspect.
        pdf: PathBuf,
        /// Directory containing the PDFium library.
        #[arg(long)]
        pdfium_dir: Option<PathBuf>,
    },
    /// Build the portable zip package.
    #[command(name = "dist:portable")]
    DistPortable,
    /// Build the NSIS setup installer.
    #[command(name = "dist:nsis")]
    DistNsis,
    /// Build the Inno Setup installer.
    #[command(name = "dist:inno")]
    DistInno,
    /// Build (and by default sign) the MSIX package.
    #[command(name = "dist:msix")]
    DistMsix {
        /// Skip signing with the self-signed certificate.
        #[arg(long)]
        no_sign: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::FetchPdfium { target, force } => {
            let triple = target.map_or_else(host_triple, Ok)?;
            let staged = fetch_pdfium(&triple, force)?;
            println!("staged {}", staged.display());
        }
        Cmd::Build { release } => {
            fetch_pdfium(&host_triple()?, false)?;
            cargo(&profile_args(release))?;
        }
        Cmd::Run { release, args } => {
            fetch_pdfium(&host_triple()?, false)?;
            cargo(&profile_args(release))?;

            let profile = if release { "release" } else { "debug" };
            let binary = PathBuf::from("target").join(profile).join(binary_name());

            let status = Command::new(&binary)
                .args(args)
                .status()
                .with_context(|| format!("running {}", binary.display()))?;

            if !status.success() {
                bail!("application exited with {status}");
            }
        }
        Cmd::Inspect { pdf, pdfium_dir } => {
            cargo(&["build", "-p", "pdfreader-pdf", "--example", "inspect"])?;

            let dir = match pdfium_dir {
                Some(dir) => dir,
                None => staged_dir(&host_triple()?),
            };

            let status = Command::new(example_binary("inspect"))
                .arg(&pdf)
                .arg("--pdfium-dir")
                .arg(&dir)
                .status()
                .context("running inspect example")?;

            if !status.success() {
                bail!("inspect failed");
            }
        }
        Cmd::DistPortable => {
            dist::portable()?;
        }
        Cmd::DistNsis => {
            dist::nsis()?;
        }
        Cmd::DistInno => {
            dist::inno()?;
        }
        Cmd::DistMsix { no_sign } => {
            dist::msix(no_sign)?;
        }
    }

    Ok(())
}

/// Run cargo from the workspace root.
fn cargo(args: &[&str]) -> Result<()> {
    let status = Command::new(cargo_bin())
        .args(args)
        .status()
        .with_context(|| format!("running cargo {}", args.join(" ")))?;

    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
}

/// Cargo arguments for a debug or release build.
fn profile_args(release: bool) -> Vec<&'static str> {
    if release {
        vec!["build", "--release", "-p", "pdfreader-app"]
    } else {
        vec!["build", "-p", "pdfreader-app"]
    }
}

/// The cargo executable, honouring `CARGO` when run as `cargo xtask`.
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Host target triple as reported by rustc.
fn host_triple() -> Result<String> {
    if let Ok(triple) = std::env::var("XTASK_TARGET") {
        return Ok(triple);
    }

    // Ask rustc directly; `cargo rustc` needs a package context and refuses.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());

    let output = Command::new(rustc)
        .args(["-vV"])
        .output()
        .context("querying rustc for the host triple")?;

    let text = String::from_utf8_lossy(&output.stdout);

    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .map(ToString::to_string)
        .context("rustc did not report a host triple")
}

/// Map a Rust target triple onto a `bblanchon/pdfium-binaries` asset name.
fn pdfium_asset(triple: &str) -> Result<&'static str> {
    match triple {
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => Ok("pdfium-win-x64"),
        "aarch64-pc-windows-msvc" => Ok("pdfium-win-arm64"),
        "x86_64-unknown-linux-gnu" => Ok("pdfium-linux-x64"),
        "aarch64-unknown-linux-gnu" => Ok("pdfium-linux-arm64"),
        "x86_64-apple-darwin" => Ok("pdfium-mac-x64"),
        "aarch64-apple-darwin" => Ok("pdfium-mac-arm64"),
        other => bail!("no pinned PDFium build for target `{other}`"),
    }
}

/// File name of the PDFium library on a given triple.
fn pdfium_lib_name(triple: &str) -> &'static str {
    if triple.contains("windows") {
        "pdfium.dll"
    } else if triple.contains("apple") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Directory the staged library is written to for a triple.
fn staged_dir(triple: &str) -> PathBuf {
    PathBuf::from("target").join("native").join(triple)
}

/// Download, verify and extract PDFium for a target.
fn fetch_pdfium(triple: &str, force: bool) -> Result<PathBuf> {
    let asset = pdfium_asset(triple)?;
    let lib_name = pdfium_lib_name(triple);
    let dir = staged_dir(triple);
    let lib_path = dir.join(lib_name);

    if lib_path.is_file() && !force {
        return Ok(dir);
    }

    fs::create_dir_all(&dir)?;

    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_TAG}/{asset}.tgz"
    );
    let archive_path = dir.join(format!("{asset}.tgz"));

    println!("fetching PDFium {PDFIUM_VERSION} for {triple}");
    println!("  {url}");

    let bytes = download(&url)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));

    fs::write(&archive_path, &bytes)?;
    verify_digest(&archive_path, &digest)?;
    extract(&archive_path, &dir)?;

    let extracted = dir.join("bin").join(lib_name);
    if !extracted.is_file() {
        bail!("archive did not contain bin/{lib_name}; upstream layout may have changed");
    }

    if extracted != lib_path {
        fs::copy(&extracted, &lib_path)?;
    }

    // Put the library beside the binaries so the app finds it at runtime with
    // no env var and no installer step.
    for profile in ["debug", "release"] {
        let profile_dir = PathBuf::from("target").join(profile);
        if profile_dir.is_dir() {
            fs::copy(&lib_path, profile_dir.join(lib_name))?;
        }
    }

    println!("  sha256 {digest}");
    Ok(dir)
}

/// Trust-on-first-use digest pinning.
///
/// The first fetch records the digest; later fetches must match. Weaker than
/// committing digests for all six targets up front — Phase 10 should replace
/// this with a committed manifest — but it still catches a substituted or
/// corrupted download, which is the failure that matters.
fn verify_digest(archive: &Path, digest: &str) -> Result<()> {
    let lock = archive.with_extension("sha256");

    match fs::read_to_string(&lock) {
        Ok(recorded) if recorded.trim() == digest => Ok(()),
        Ok(recorded) => bail!(
            "PDFium archive digest mismatch.\n  recorded: {}\n  actual:   {}\n\
             Delete {} to accept a new build deliberately.",
            recorded.trim(),
            digest,
            lock.display()
        ),
        Err(_) => {
            fs::write(&lock, format!("{digest}\n"))?;
            Ok(())
        }
    }
}

/// Fetch a URL over TLS.
fn download(url: &str) -> Result<Vec<u8>> {
    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;

    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .context("reading response body")?;

    if bytes.is_empty() {
        bail!("empty response from {url}");
    }

    Ok(bytes)
}

/// Extract a gzipped tarball into `dest`.
fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    tar::Archive::new(flate2::read::GzDecoder::new(file)).unpack(dest)?;
    Ok(())
}

/// Name of the application binary on this platform.
fn binary_name() -> String {
    if cfg!(windows) {
        "pdf-reader.exe".to_string()
    } else {
        "pdf-reader".to_string()
    }
}

/// Path of a built example binary.
fn example_binary(name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    PathBuf::from("target")
        .join("debug")
        .join("examples")
        .join(file)
}
