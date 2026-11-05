//! Rules for which folders may become library folders (CONCEPT.md §3.1).
//!
//! - Symlinks are not allowed: neither the folder itself nor a symlinked
//!   parent inside the user's home (e.g. `~/Musik/Verknüpfung mit Musik@GDrive/…`).
//!   Symlinks at or above the home folder (`/home → /var/home` on Atomic
//!   desktops) are system layout and are ignored.
//! - Library folders must not overlap: no folder inside another one, and no
//!   folder containing another one. Folders are compared by canonical path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Why a folder can't be added to the library.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FolderError {
    #[error("folder does not exist")]
    NotFound,
    #[error("not a folder")]
    NotADirectory,
    #[error("path is not valid UTF-8")]
    NotUtf8,
    #[error("symlinks are not allowed ({})", .link.display())]
    Symlink { link: PathBuf },
    #[error("already a library folder")]
    AlreadyAdded,
    #[error("inside the library folder {}", .0.display())]
    InsideExisting(PathBuf),
    #[error("contains the library folder {}", .0.display())]
    ContainsExisting(PathBuf),
    #[error("cannot read folder: {0}")]
    Io(String),
}

impl From<io::Error> for FolderError {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::NotADirectory => Self::NotADirectory,
            _ => Self::Io(err.to_string()),
        }
    }
}

/// Checks whether `candidate` may be added next to the `existing` library
/// folders and returns the canonical path to store.
///
/// `home` is the user's home folder; symlinks at or above it are ignored.
pub fn validate_new_folder(
    candidate: &Path,
    existing: &[PathBuf],
    home: Option<&Path>,
) -> Result<PathBuf, FolderError> {
    if candidate.to_str().is_none() {
        return Err(FolderError::NotUtf8);
    }
    if let Some(link) = find_symlink(candidate, home)? {
        return Err(FolderError::Symlink { link });
    }
    if !fs::metadata(candidate)?.is_dir() {
        return Err(FolderError::NotADirectory);
    }
    let canonical = fs::canonicalize(candidate)?;
    if canonical.to_str().is_none() {
        return Err(FolderError::NotUtf8);
    }

    for folder in existing {
        // A library folder that's currently missing (unmounted drive) is
        // compared as stored.
        let other = fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        if other == canonical {
            return Err(FolderError::AlreadyAdded);
        }
        if canonical.starts_with(&other) {
            return Err(FolderError::InsideExisting(folder.clone()));
        }
        if other.starts_with(&canonical) {
            return Err(FolderError::ContainsExisting(folder.clone()));
        }
    }
    Ok(canonical)
}

/// Returns the first symlink among `path` itself and its parents inside `home`.
fn find_symlink(path: &Path, home: Option<&Path>) -> Result<Option<PathBuf>, FolderError> {
    for (depth, ancestor) in path.ancestors().enumerate() {
        let inside_home = home.is_some_and(|home| ancestor != home && ancestor.starts_with(home));
        if depth > 0 && !inside_home {
            // Everything further up is at or above home (or unrelated to it).
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(Some(ancestor.to_path_buf())),
            Ok(_) => {}
            // A missing path is reported by the caller's metadata check.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    /// A fake home folder with `Musik/` inside.
    struct Home {
        _dir: TempDir,
        home: PathBuf,
        music: PathBuf,
    }

    fn home() -> Home {
        let dir = TempDir::new().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap().join("home");
        let music = home.join("Musik");
        fs::create_dir_all(&music).unwrap();
        Home {
            _dir: dir,
            home,
            music,
        }
    }

    fn validate(h: &Home, candidate: &Path, existing: &[PathBuf]) -> Result<PathBuf, FolderError> {
        validate_new_folder(candidate, existing, Some(&h.home))
    }

    #[test]
    fn accepts_plain_folder() {
        let h = home();
        assert_eq!(validate(&h, &h.music, &[]), Ok(h.music.clone()));
    }

    #[test]
    fn returns_canonical_path() {
        let h = home();
        let messy = h.music.join("..").join("Musik").join(".");
        assert_eq!(validate(&h, &messy, &[]), Ok(h.music.clone()));
    }

    #[test]
    fn rejects_symlinked_folder() {
        let h = home();
        let drive = h.home.join("Google Drive/Musik");
        fs::create_dir_all(&drive).unwrap();
        let link = h.home.join("Musik@GDrive");
        symlink(&drive, &link).unwrap();
        assert_eq!(
            validate(&h, &link, &[]),
            Err(FolderError::Symlink { link: link.clone() })
        );
    }

    #[test]
    fn rejects_folder_below_symlinked_parent_inside_home() {
        let h = home();
        let drive = h.home.join("Google Drive/Musik");
        fs::create_dir_all(drive.join("Adele")).unwrap();
        let link = h.music.join("Verknüpfung mit Musik@GDrive");
        symlink(&drive, &link).unwrap();
        assert_eq!(
            validate(&h, &link.join("Adele"), &[]),
            Err(FolderError::Symlink { link })
        );
    }

    #[test]
    fn ignores_symlinks_at_or_above_home() {
        // Like /home → /var/home on Fedora Atomic desktops.
        let h = home();
        let linked_home = h.home.with_file_name("linked-home");
        symlink(&h.home, &linked_home).unwrap();
        let candidate = linked_home.join("Musik");
        assert_eq!(
            validate_new_folder(&candidate, &[], Some(&linked_home)),
            Ok(h.music.clone())
        );
    }

    #[test]
    fn rejects_missing_folder_and_files() {
        let h = home();
        assert_eq!(
            validate(&h, &h.home.join("nope"), &[]),
            Err(FolderError::NotFound)
        );
        let file = h.music.join("song.flac");
        fs::write(&file, b"").unwrap();
        assert_eq!(validate(&h, &file, &[]), Err(FolderError::NotADirectory));
    }

    #[test]
    fn rejects_overlapping_folders() {
        let h = home();
        let artist = h.music.join("Adele");
        fs::create_dir_all(&artist).unwrap();

        assert_eq!(
            validate(&h, &h.music, std::slice::from_ref(&h.music)),
            Err(FolderError::AlreadyAdded)
        );
        assert_eq!(
            validate(&h, &artist, std::slice::from_ref(&h.music)),
            Err(FolderError::InsideExisting(h.music.clone()))
        );
        assert_eq!(
            validate(&h, &h.music, std::slice::from_ref(&artist)),
            Err(FolderError::ContainsExisting(artist))
        );
    }

    #[test]
    fn sibling_with_common_prefix_is_not_inside() {
        // Path::starts_with compares components, so "Musik2" isn't inside "Musik".
        let h = home();
        let sibling = h.home.join("Musik2");
        fs::create_dir_all(&sibling).unwrap();
        assert_eq!(
            validate(&h, &sibling, std::slice::from_ref(&h.music)),
            Ok(sibling)
        );
    }

    #[test]
    fn missing_existing_folder_is_compared_as_stored() {
        let h = home();
        let unmounted = PathBuf::from("/run/media/nobody/Music");
        assert_eq!(validate(&h, &h.music, &[unmounted]), Ok(h.music.clone()));
    }
}
