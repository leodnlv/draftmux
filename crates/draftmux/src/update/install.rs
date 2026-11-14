//! Replaces the installed app with a release's: `Draftmux.app` on macOS and
//! the app tree the install script puts in `lib/draftmux` on Linux. The new
//! copy is downloaded and checked in a folder beside the installed one, so
//! the swap is one atomic rename on the same filesystem. The running app
//! keeps its old executable until it restarts.

#[cfg_attr(target_os = "macos", path = "install/macos.rs")]
#[cfg_attr(target_os = "linux", path = "install/linux.rs")]
mod platform;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

pub use platform::{Installation, installation};

use super::{Release, release};

const TOOL_TIMEOUT: Duration = Duration::from_secs(2 * 60);

pub fn install(release: &Release, installation: &Installation) -> Result<()> {
    let installed = installation.path();
    let dir = installed
        .parent()
        .with_context(|| format!("{} has no parent folder", installed.display()))?;
    // Removed with whatever it holds when this returns, including the old
    // copy after a successful swap.
    let staging = tempfile::Builder::new()
        .prefix(platform::STAGING_PREFIX)
        .tempdir_in(dir)
        .with_context(|| format!("creating a staging folder in {}", dir.display()))?;
    let archive = staging.path().join(release::archive_name(&release.version));
    release::download(&release.archive_url, &archive)?;
    let checksum = release::fetch_text(&release.checksum_url)?;
    let bytes = std::fs::read(&archive).context("reading the downloaded archive")?;
    verify_checksum(&bytes, &checksum)?;

    let extracted = staging.path().join("extracted");
    std::fs::create_dir(&extracted).context("creating the extraction folder")?;
    let mut tar = Command::new(platform::TAR);
    tar.arg("-xzf").arg(&archive).arg("-C").arg(&extracted);
    run("tar", tar)?;
    let new = only_entry(&extracted, platform::ARCHIVE_ROOT)?;
    platform::verify(&new, release, installation)?;
    swap(&new, installed)
}

/// Checks `archive` against a `shasum -a 256` or `sha256sum` line.
fn verify_checksum(archive: &[u8], checksum_file: &str) -> Result<()> {
    let expected = checksum_file
        .split_whitespace()
        .next()
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("the checksum file has no SHA-256")?;
    let actual: String = Sha256::digest(archive)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("the downloaded archive doesn't match its checksum");
    }
    Ok(())
}

/// The archive must hold the folder `name` and nothing else.
fn only_entry(extracted: &Path, name: &str) -> Result<PathBuf> {
    let names: Vec<_> = std::fs::read_dir(extracted)
        .context("reading the extracted archive")?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<_>>()
        .context("reading the extracted archive")?;
    if names != [name] {
        bail!("the archive doesn't hold just {name}");
    }
    let path = extracted.join(name);
    let metadata = std::fs::symlink_metadata(&path).context("reading the extracted app")?;
    if !metadata.is_dir() {
        bail!("{name} in the archive isn't a folder");
    }
    Ok(path)
}

/// Atomically exchanges the two folders: `installed` becomes the new one
/// and `new` the old.
#[cfg(target_os = "macos")]
fn swap(new: &Path, installed: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(new.as_os_str().as_bytes())?;
    let to = CString::new(installed.as_os_str().as_bytes())?;
    // SAFETY: both are valid NUL-terminated paths for the call's duration.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("replacing {}", installed.display()));
    }
    Ok(())
}

/// Atomically exchanges the two folders: `installed` becomes the new one
/// and `new` the old. A filesystem without `RENAME_EXCHANGE` gets two
/// renames instead, which put the installed folder back if the second
/// fails.
#[cfg(target_os = "linux")]
fn swap(new: &Path, installed: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(new.as_os_str().as_bytes())?;
    let to = CString::new(installed.as_os_str().as_bytes())?;
    // SAFETY: both are valid NUL-terminated paths for the call's duration.
    let exchanged = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if exchanged == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if !matches!(
        err.raw_os_error(),
        Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
    ) {
        return Err(err).with_context(|| format!("replacing {}", installed.display()));
    }
    swap_by_renames(new, installed)
}

#[cfg(target_os = "linux")]
fn swap_by_renames(new: &Path, installed: &Path) -> Result<()> {
    let aside = new.with_file_name(".previous");
    std::fs::rename(installed, &aside)
        .with_context(|| format!("moving {} aside", installed.display()))?;
    if let Err(err) = std::fs::rename(new, installed) {
        let _ = std::fs::rename(&aside, installed);
        return Err(err).with_context(|| format!("replacing {}", installed.display()));
    }
    std::fs::rename(&aside, new).context("keeping the previous version for removal")
}

fn run(name: &str, command: Command) -> Result<String> {
    crate::process::run(name, command, None, TOOL_TIMEOUT, None).map_err(|err| anyhow!(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_must_match() {
        let archive = b"archive";
        let sha = "4a0e6a6d6e6b0a2f1d7c7a4e0d3a36a5c1d2b3d0e4b6f0b0c9e5e6f2f5a8b9c1";
        assert!(verify_checksum(archive, &format!("{sha}  a.tar.gz")).is_err());
        let actual: String = Sha256::digest(archive)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        verify_checksum(
            archive,
            &format!("{actual}  Draftmux-1.0.0-arm64-app.tar.gz\n"),
        )
        .unwrap();
        assert!(verify_checksum(archive, "").is_err());
        assert!(verify_checksum(archive, "abc  file").is_err());
    }

    #[test]
    fn archive_must_hold_only_the_app() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("app")).unwrap();
        assert_eq!(
            only_entry(dir.path(), "app").unwrap(),
            dir.path().join("app")
        );
        assert!(only_entry(dir.path(), "other").is_err());
        std::fs::write(dir.path().join("extra"), "").unwrap();
        assert!(only_entry(dir.path(), "app").is_err());
    }

    #[test]
    fn archive_app_must_be_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/", dir.path().join("app")).unwrap();
        assert!(only_entry(dir.path(), "app").is_err());
    }

    fn assert_swaps(swap: fn(&Path, &Path) -> Result<()>) {
        let dir = tempfile::tempdir().unwrap();
        let (new, installed) = (dir.path().join("new"), dir.path().join("installed"));
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("version"), "2").unwrap();
        std::fs::create_dir(&installed).unwrap();
        std::fs::write(installed.join("version"), "1").unwrap();
        swap(&new, &installed).unwrap();
        assert_eq!(
            std::fs::read_to_string(installed.join("version")).unwrap(),
            "2"
        );
        assert_eq!(std::fs::read_to_string(new.join("version")).unwrap(), "1");
    }

    #[test]
    fn swap_exchanges_folders() {
        assert_swaps(swap);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn renames_exchange_folders() {
        assert_swaps(swap_by_renames);
    }
}
