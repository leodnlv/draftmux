//! Updates of the app tree from a Linux release, installed by its `install`
//! script in `lib/draftmux`. Only the tree is replaced: the `draftmux`
//! command and the desktop entry point into it by path. Linux releases
//! aren't signed, so a release is trusted as far as GitHub's HTTPS and its
//! checksum file, as by the install script.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::run;
use crate::resources::LINUX_MARKER;
use crate::update::{Ineligible, Release};

/// In the root of a tree that scripts/release-linux built, holding its
/// version. Trees without it are development builds.
const RELEASE_MARKER: &str = ".draftmux-release";

pub const TAR: &str = "tar";
pub const ARCHIVE_ROOT: &str = "draftmux-linux";
pub const STAGING_PREFIX: &str = ".draftmux-update-";

/// The running app tree, which updates replace.
pub struct Installation {
    root: PathBuf,
}

/// Whether this copy can update itself: a release tree in a folder the user
/// can write to.
pub fn installation() -> Result<Installation, Ineligible> {
    let exe = crate::resources::exe().map_err(|_| Ineligible::Development)?;
    let root = crate::resources::linux_root(&exe)
        .ok_or(Ineligible::Development)?
        .to_owned();
    if !root.join(RELEASE_MARKER).is_file() {
        return Err(Ineligible::Development);
    }
    let dir = root.parent().ok_or(Ineligible::Development)?;
    let writable = tempfile::Builder::new()
        .prefix(".draftmux-write-test-")
        .tempfile_in(dir)
        .is_ok();
    if !writable {
        return Err(Ineligible::Unwritable(dir.display().to_string()));
    }
    Ok(Installation { root })
}

impl Installation {
    pub fn path(&self) -> &Path {
        &self.root
    }
}

/// Checks the extracted tree is a complete release tree whose binary runs
/// here and reports `release`'s version, and gives it the installed tree's
/// permissions.
pub fn verify(new: &Path, release: &Release, installation: &Installation) -> Result<()> {
    check_tree(new)?;
    let mut version = Command::new(new.join("bin/draftmux"));
    version.arg("--version");
    let printed =
        run("the new draftmux", version).context("the new draftmux doesn't run on this system")?;
    let expected = format!("draftmux {}", release.version);
    if printed.trim() != expected {
        bail!(
            "the archive holds {:?}, not Draftmux {}",
            printed.trim(),
            release.version
        );
    }
    let mode = std::fs::metadata(&installation.root)
        .context("reading the installed app's permissions")?
        .permissions()
        .mode();
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(mode & 0o7777))
        .context("setting the new app's permissions")
}

/// The tree must have the markers and the pi extension that the install
/// script requires, and an executable binary rather than a link out of it.
fn check_tree(new: &Path) -> Result<()> {
    for marker in [LINUX_MARKER, RELEASE_MARKER] {
        if !new.join(marker).is_file() {
            bail!("the archive's app has no {marker}");
        }
    }
    if !new
        .join("share/draftmux/pi-extension/draftmux.ts")
        .is_file()
    {
        bail!("the archive's app has no pi extension");
    }
    let binary = std::fs::symlink_metadata(new.join("bin/draftmux"))
        .context("the archive's app has no bin/draftmux")?;
    if !binary.is_file() || binary.permissions().mode() & 0o100 == 0 {
        bail!("bin/draftmux in the archive isn't an executable file");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn tree(dir: &Path) -> PathBuf {
        let root = dir.join(ARCHIVE_ROOT);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("share/draftmux/pi-extension")).unwrap();
        fs::write(root.join("share/draftmux/pi-extension/draftmux.ts"), "").unwrap();
        fs::write(root.join(LINUX_MARKER), "draftmux\n").unwrap();
        fs::write(root.join(RELEASE_MARKER), "1.2.3\n").unwrap();
        let binary = root.join("bin/draftmux");
        fs::write(&binary, "#!/bin/sh\necho draftmux 1.2.3\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        root
    }

    #[test]
    fn release_tree_is_complete() {
        let dir = tempfile::tempdir().unwrap();
        check_tree(&tree(dir.path())).unwrap();
    }

    #[test]
    fn tree_without_release_marker_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = tree(dir.path());
        fs::remove_file(root.join(RELEASE_MARKER)).unwrap();
        assert!(check_tree(&root).is_err());
    }

    #[test]
    fn binary_must_be_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = tree(dir.path());
        let binary = root.join("bin/draftmux");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_tree(&root).is_err());
        fs::remove_file(&binary).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &binary).unwrap();
        assert!(check_tree(&root).is_err());
    }

    #[test]
    fn verify_runs_the_new_binary_and_copies_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let root = tree(dir.path());
        let installed = dir.path().join("installed");
        fs::create_dir(&installed).unwrap();
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o750)).unwrap();
        let installation = Installation { root: installed };
        let release = |version: semver::Version| Release {
            version,
            page: String::new(),
            archive_url: String::new(),
            checksum_url: String::new(),
            notes: None,
        };
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        verify(
            &root,
            &release(semver::Version::new(1, 2, 3)),
            &installation,
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o750
        );
        let err = verify(
            &root,
            &release(semver::Version::new(1, 2, 4)),
            &installation,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("1.2.4"), "{err}");
    }
}
