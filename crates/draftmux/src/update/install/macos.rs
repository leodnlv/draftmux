//! Updates of a Developer ID signed `Draftmux.app`. A release's bundle must
//! carry its version and be signed by the same team as the installed one.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use semver::Version;

use super::run;
use crate::update::{BUNDLE_ID, Ineligible, Release};

pub const TAR: &str = "/usr/bin/tar";
pub const ARCHIVE_ROOT: &str = "Draftmux.app";
pub const STAGING_PREFIX: &str = ".Draftmux-update-";

/// The running `Draftmux.app`, which updates replace.
pub struct Installation {
    app: PathBuf,
    /// The Developer ID team that signed it; a release must be signed by the
    /// same team.
    team: String,
}

/// Whether this copy can update itself: a Developer ID signed bundle in a
/// folder the user can write to.
pub fn installation() -> Result<Installation, Ineligible> {
    let exe = crate::resources::exe().map_err(|_| Ineligible::Development)?;
    let app = crate::resources::bundle(&exe)
        .ok_or(Ineligible::Development)?
        .to_owned();
    let team = signing_team(&app).ok_or(Ineligible::Development)?;
    let dir = app.parent().ok_or(Ineligible::Development)?;
    let writable = tempfile::Builder::new()
        .prefix(".Draftmux-write-test-")
        .tempfile_in(dir)
        .is_ok();
    if !writable {
        return Err(Ineligible::Unwritable(dir.display().to_string()));
    }
    Ok(Installation { app, team })
}

impl Installation {
    pub fn path(&self) -> &Path {
        &self.app
    }
}

/// Checks the extracted bundle is `release`'s, signed by the installed
/// app's team.
pub fn verify(new_app: &Path, release: &Release, installation: &Installation) -> Result<()> {
    let version = bundle_version(new_app)?;
    if version != release.version {
        bail!(
            "the archive holds Draftmux {version}, not {}",
            release.version
        );
    }
    verify_signature(new_app, &installation.team)
}

fn bundle_version(app: &Path) -> Result<Version> {
    let mut plutil = Command::new("/usr/bin/plutil");
    plutil
        .args(["-extract", "CFBundleShortVersionString", "raw"])
        .arg(app.join("Contents/Info.plist"));
    let raw = run("plutil", plutil)?;
    Version::parse(raw.trim()).with_context(|| format!("the bundle's version {raw:?} isn't valid"))
}

/// Requires a valid, complete signature by `team` for Draftmux's identifier.
/// The signature seals every file in the bundle.
fn verify_signature(app: &Path, team: &str) -> Result<()> {
    let requirement = format!(
        "=anchor apple generic and identifier \"{BUNDLE_ID}\" \
         and certificate leaf[subject.OU] = \"{team}\""
    );
    let mut codesign = Command::new("/usr/bin/codesign");
    codesign
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R{requirement}"))
        .arg(app);
    run("codesign", codesign)
        .context("the new Draftmux.app isn't signed by Draftmux's developer")?;
    Ok(())
}

/// The Developer ID team of a bundle signed with Draftmux's identifier;
/// `None` for ad-hoc signed development builds.
fn signing_team(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    // codesign prints the details on stderr.
    parse_signing_team(&String::from_utf8_lossy(&output.stderr))
}

fn parse_signing_team(details: &str) -> Option<String> {
    let field = |name: &str| {
        details
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
    };
    let team = field("TeamIdentifier")?;
    let developer_id = details
        .lines()
        .any(|line| line.starts_with("Authority=Developer ID Application:"));
    (field("Identifier") == Some(BUNDLE_ID)
        && developer_id
        && !team.is_empty()
        && team.bytes().all(|b| b.is_ascii_alphanumeric()))
    .then(|| team.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_DETAILS: &str = "Executable=/Applications/Draftmux.app/Contents/MacOS/draftmux
Identifier=fi.zendit.Draftmux
Format=app bundle with Mach-O thin (arm64)
Authority=Developer ID Application: Example Person (ABCDE12345)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
TeamIdentifier=ABCDE12345
";

    #[test]
    fn developer_id_builds_have_a_team() {
        assert_eq!(
            parse_signing_team(RELEASE_DETAILS).as_deref(),
            Some("ABCDE12345")
        );
    }

    #[test]
    fn development_and_foreign_builds_have_no_team() {
        let adhoc = "Identifier=fi.zendit.Draftmux\nSignature=adhoc\nTeamIdentifier=not set\n";
        assert_eq!(parse_signing_team(adhoc), None);
        let other = RELEASE_DETAILS.replace("fi.zendit.Draftmux", "com.example.Other");
        assert_eq!(parse_signing_team(&other), None);
        assert_eq!(parse_signing_team(""), None);
    }
}
