//! Filesystem path helpers.

use std::io;
use std::path::{Path, PathBuf};

/// `std::fs::canonicalize` without the Windows verbatim prefix.
///
/// On Windows `std::fs::canonicalize` returns `\\?\C:\...` (or `\\?\UNC\...`)
/// paths. Those are valid but ugly, and they leak into `state.toml`, the
/// compiled shell files and printed output. This strips the prefix whenever
/// the result is still a valid legacy path. On other platforms it is plain
/// `std::fs::canonicalize`.
pub fn canonicalize(p: impl AsRef<Path>) -> io::Result<PathBuf> {
    dunce::canonicalize(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERBATIM: &str = r"\\?\";

    #[test]
    fn strips_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let got = canonicalize(dir.path()).unwrap();
        let s = got.to_string_lossy();
        assert!(!s.starts_with(VERBATIM), "verbatim prefix leaked: {s}");
        assert!(got.is_absolute());
        assert!(got.is_dir());
        // Still the same directory as std would resolve.
        assert_eq!(
            std::fs::canonicalize(&got).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn resolves_relative_components() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let got = canonicalize(dir.path().join("sub").join("..").join("sub")).unwrap();
        assert!(got.ends_with("sub"));
        assert!(!got.to_string_lossy().contains(".."));
    }

    #[cfg(windows)]
    #[test]
    fn accepts_verbatim_input() {
        let dir = tempfile::tempdir().unwrap();
        let verbatim = std::fs::canonicalize(dir.path()).unwrap();
        assert!(verbatim.to_string_lossy().starts_with(VERBATIM));
        let got = canonicalize(&verbatim).unwrap();
        assert!(!got.to_string_lossy().starts_with(VERBATIM));
    }

    #[test]
    fn missing_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(canonicalize(dir.path().join("nope")).is_err());
    }
}
