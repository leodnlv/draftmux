//! Windows releases aren't replaced in place: the running executable can't
//! be renamed over, and a release is installed by hand from its download.

use anyhow::Result;

use super::{Ineligible, Release};
use crate::resources::RELEASE_MARKER;

/// No copy on Windows installs updates itself.
pub enum Installation {}

/// A release tree is told to download the update; anything else is a
/// development build.
pub fn installation() -> Result<Installation, Ineligible> {
    let exe = crate::resources::exe().map_err(|_| Ineligible::Development)?;
    let root = crate::resources::tree_root(&exe).ok_or(Ineligible::Development)?;
    if root.join(RELEASE_MARKER).is_file() {
        Err(Ineligible::Manual)
    } else {
        Err(Ineligible::Development)
    }
}

pub fn install(_: &Release, installation: &Installation) -> Result<()> {
    match *installation {}
}
