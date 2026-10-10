//! Reading the bundled dictionary files off disk.
//!
//! This is the only part of the crate that does I/O. The application calls
//! [`load`] once, on a background thread, and then shares the resulting
//! [`Dictionaries`] immutably for the rest of the session.

use std::io::Error as IoError;
use std::path::{Path, PathBuf};

use crate::dict::{Dictionaries, ParseError, parse_bangla, parse_e2b, parse_ipa};

/// The files a dictionary directory must contain, in load order.
pub const FILES: [&str; 3] = [
    "E2Bdatabase.json",
    "bangla_dictionary.txt",
    "cmudict-0.7b-ipa.txt",
];

/// Why the dictionaries could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// A file was missing or unreadable.
    #[error("could not read {file}: {source}")]
    Read {
        /// Name of the file that could not be read.
        file: String,
        /// The underlying I/O failure.
        #[source]
        source: IoError,
    },
    /// A file was read but had no usable content.
    #[error("{0}")]
    Parse(#[from] ParseError),
}

/// Load all three dictionaries from a directory.
///
/// `dir` is the folder holding [`FILES`] — next to the executable in a shipped
/// build, or `assets/lens` when running from the repository.
pub fn load(dir: &Path) -> Result<Dictionaries, LoadError> {
    let json = read(dir, FILES[0])?;
    let bangla = read(dir, FILES[1])?;
    let ipa = read(dir, FILES[2])?;

    Ok(Dictionaries::new(
        parse_e2b(&json)?,
        parse_bangla(&bangla),
        parse_ipa(&ipa),
    ))
}

/// Where the dictionaries should be looked for, given the executable's folder.
///
/// Checks beside the executable first, then `assets/lens` relative to the
/// current directory, which is how a `cargo run` from the repository root
/// finds them without a copy step.
pub fn candidate_dirs(exe_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(exe_dir) = exe_dir {
        dirs.push(exe_dir.join("lens"));
        dirs.push(exe_dir);
    }
    dirs.push(PathBuf::from("assets/lens"));
    dirs
}

/// The first candidate directory that actually holds the dictionaries.
pub fn find_dir(exe_dir: Option<PathBuf>) -> Option<PathBuf> {
    candidate_dirs(exe_dir)
        .into_iter()
        .find(|dir| files_present(dir))
}

/// Whether every dictionary file exists in `dir`.
fn files_present(dir: &Path) -> bool {
    FILES.iter().all(|file| dir.join(file).is_file())
}

fn read(dir: &Path, file: &str) -> Result<String, LoadError> {
    let path = dir.join(file);
    std::fs::read_to_string(&path).map_err(|source| LoadError::Read {
        file: file.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_ordered_from_shipped_to_source() {
        let dirs = candidate_dirs(Some(PathBuf::from("dist")));
        assert_eq!(dirs[0], PathBuf::from("dist/lens"));
        assert_eq!(dirs[1], PathBuf::from("dist"));
        assert_eq!(dirs.last(), Some(&PathBuf::from("assets/lens")));
    }

    #[test]
    fn a_directory_missing_files_is_not_a_candidate() {
        assert!(!files_present(Path::new(".")));
    }

    #[test]
    fn a_missing_file_reports_its_name() {
        let error = load(Path::new("no/such/dir")).expect_err("nothing to load");
        assert!(matches!(error, LoadError::Read { .. }));
        assert!(error.to_string().contains("E2Bdatabase.json"));
    }
}
