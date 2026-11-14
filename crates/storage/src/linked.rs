//! A draft's linked file: the file it was opened from, which saving writes
//! the draft back to.

use std::io;

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::meta::{LinkedFile, Meta};

/// SHA-256 of `bytes` in lowercase hex, as [`LinkedFile::sha256`] holds it.
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Where a draft stands against its linked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileState {
    /// The draft's text is what the draft last read from or wrote to the
    /// file.
    pub saved: bool,
    /// The file no longer holds what the draft last read or wrote: it was
    /// changed or removed outside the draft.
    pub changed_on_disk: bool,
}

/// The result of [`crate::Store::save_to_file`].
#[derive(Debug)]
pub enum FileSave {
    Saved(Box<Meta>),
    /// The file was changed outside the draft, and was left alone.
    Changed,
    /// The file was removed outside the draft, and wasn't recreated.
    Missing,
}

/// What the linked file holds compared with what the draft last read or
/// wrote.
pub(crate) enum OnDisk {
    Same,
    Changed,
    Missing,
}

impl LinkedFile {
    /// Links `path`, whose contents the draft starts with.
    pub fn new(path: impl Into<std::path::PathBuf>, contents: &[u8]) -> Self {
        Self {
            path: path.into(),
            sha256: sha256(contents),
        }
    }

    /// Where a draft holding `text` stands against the file.
    pub fn state(&self, text: &str) -> Result<FileState> {
        Ok(FileState {
            saved: sha256(text.as_bytes()) == self.sha256,
            changed_on_disk: !matches!(self.on_disk()?, OnDisk::Same),
        })
    }

    pub(crate) fn on_disk(&self) -> Result<OnDisk> {
        match std::fs::read(&self.path) {
            Ok(contents) if sha256(&contents) == self.sha256 => Ok(OnDisk::Same),
            Ok(_) => Ok(OnDisk::Changed),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(OnDisk::Missing),
            Err(err) => Err(err).with_context(|| format!("reading {}", self.path.display())),
        }
    }
}
