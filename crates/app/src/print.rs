//! Handing a PDF to the operating system's print pipeline.
//!
//! There is no silent printing here, and no way to add one: the shell's
//! `print` verb always shows the printer dialog. What it buys is a real print
//! job from a real PDF file, which is the only way the document renders the way
//! it looks on screen.

use std::path::{Path, PathBuf};

/// Prefix shared by every print spool file this application creates.
///
/// The sweep matches on the prefix alone, so a file left by a crashed run with
/// a different process id is still cleaned up.
const SPOOL_PREFIX: &str = "pdf-reader-print-";

/// Where the spool file for one print job lives.
///
/// The process id keeps two running copies of the application from colliding,
/// and the job number keeps two prints in one session apart.
pub fn spool_path(job: u64) -> PathBuf {
    std::env::temp_dir().join(format!("{SPOOL_PREFIX}{}-{job}.pdf", std::process::id()))
}

/// Whether a file name in the temp directory is one of our spool files.
///
/// Split out from the sweep so the matching rule can be tested without
/// creating and deleting files in the real temp directory.
fn is_spool_file(name: &str) -> bool {
    if !name.starts_with(SPOOL_PREFIX) {
        return false;
    }
    // Case-insensitive: the file is ours either way, and a case-only mismatch
    // would otherwise leave it in the temp directory forever.
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// Delete spool files left behind by earlier runs.
///
/// A spool file cannot be deleted when the print job finishes: the shell
/// returns as soon as it has launched the handler, which reads the file
/// afterwards. The next launch is the first moment at which every earlier
/// print is certainly done with its file.
///
/// Returns how many files were removed, for logging. A file that is still
/// being read fails to delete and is left alone, which is correct — a later
/// launch will collect it.
pub fn sweep_stale_spool_files() -> usize {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_spool_file(name) {
            removed += usize::from(std::fs::remove_file(entry.path()).is_ok());
        }
    }
    removed
}

/// Ask the operating system to print the PDF at `path`.
///
/// Returns as soon as the shell accepts the request, which is *before* anything
/// has been printed. The caller must therefore keep `path` alive; use
/// [`spool_path`] and let [`sweep_stale_spool_files`] collect it later.
#[cfg(windows)]
pub fn print_pdf(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// `ShellExecuteW` returns a value above 32 on success. Everything at or
    /// below 32 is an error code (0 = out of memory, 5 = access denied, …).
    const SUCCESS_THRESHOLD: isize = 32;

    /// NUL-terminated UTF-16, the only string shape the wide Win32 API takes.
    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    let file = wide(path.as_os_str());
    let verb = wide(std::ffi::OsStr::new("print"));

    // SAFETY: `verb` and `file` are NUL-terminated UTF-16 buffers that outlive
    // the call, and the three remaining string arguments are documented as
    // optional, so null is valid for all of them.
    #[allow(unsafe_code)]
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    let code = result as isize;
    if code > SUCCESS_THRESHOLD {
        Ok(())
    } else {
        Err(format!(
            "the system print handler refused the request (code {code})"
        ))
    }
}

/// Ask the operating system to print the PDF at `path`.
///
/// Only Windows has a shell print verb, so every other platform reports that
/// printing is unavailable rather than pretending to have done something.
#[cfg(not(windows))]
pub fn print_pdf(_path: &Path) -> Result<(), String> {
    Err("printing is only supported on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_paths_are_named_so_the_sweep_can_find_them() {
        let path = spool_path(7);
        let name = path.file_name().expect("spool path has a name");
        let name = name.to_string_lossy();

        assert!(name.starts_with(SPOOL_PREFIX), "got {name}");
        assert!(name.ends_with(".pdf"), "got {name}");
        assert!(name.contains(&std::process::id().to_string()), "got {name}");
        assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(is_spool_file(&name));
    }

    /// The sweep runs over the whole temp directory, so it must not delete
    /// anything that is not ours. A real temp file with a similar name would
    /// be an unrecoverable loss.
    #[test]
    fn the_sweep_only_claims_our_own_files() {
        assert!(is_spool_file("pdf-reader-print-1234-7.pdf"));
        assert!(is_spool_file("pdf-reader-print-1234-7.PDF"));

        assert!(!is_spool_file("pdf-reader-print-1234-7.pdf.txt"));
        assert!(!is_spool_file("pdf-reader-print-1234-7"));
        assert!(!is_spool_file("notes.pdf"));
        assert!(!is_spool_file("pdf-reader-print-.pdfx"));
        assert!(!is_spool_file(""));
    }
}
