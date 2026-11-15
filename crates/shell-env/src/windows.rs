//! The environment on Windows: the app's own, with `PATH` brought up to date
//! from the registry.
//!
//! A process gets its environment from the one that started it, so an app
//! started by a long-running process, such as an agent's shell, misses
//! `PATH` entries that installers added since. Windows keeps the saved
//! `PATH` in the registry, where new processes started by Explorer read it.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};

use crate::Vars;

/// The process environment, with the registry's `PATH` directories that it
/// lacks added at the end of `PATH`.
pub(crate) fn env() -> Vars {
    let mut vars: Vars = std::env::vars_os().collect();
    let saved = [
        (
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
        (HKEY_CURRENT_USER, "Environment"),
    ]
    .into_iter()
    .filter_map(|(key, subkey)| read_path(key, subkey))
    .collect::<Vec<_>>();
    match vars
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
    {
        Some((_, path)) => *path = add_paths(path, &saved),
        None => vars.push(("Path".into(), add_paths(&OsString::new(), &saved))),
    }
    vars
}

/// `path` followed by the directories of `more` that it doesn't list,
/// compared the way Windows compares paths: ignoring case and trailing
/// separators.
fn add_paths(path: &OsString, more: &[OsString]) -> OsString {
    let key = |dir: &Path| {
        dir.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    let mut seen: Vec<String> = dirs.iter().map(|d| key(d)).collect();
    for dir in more.iter().flat_map(std::env::split_paths) {
        if dir.as_os_str().is_empty() || seen.contains(&key(&dir)) {
            continue;
        }
        seen.push(key(&dir));
        dirs.push(dir);
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| path.clone())
}

/// The `Path` value under `subkey`, with `%VARIABLES%` expanded.
fn read_path(key: HKEY, subkey: &str) -> Option<OsString> {
    let wide = |s: &str| {
        OsString::from(s)
            .encode_wide()
            .chain([0])
            .collect::<Vec<u16>>()
    };
    let (subkey, value) = (wide(subkey), wide("Path"));
    let mut buf = vec![0u16; 4096];
    loop {
        let mut len = u32::try_from(buf.len() * 2).ok()?;
        // SAFETY: the names are NUL-terminated and `len` is `buf`'s size in
        // bytes. RRF_RT_REG_SZ also accepts REG_EXPAND_SZ and expands it.
        let status = unsafe {
            RegGetValueW(
                key,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        match status {
            ERROR_SUCCESS => {
                // An expanded value's reported size can run past its
                // terminating NUL.
                buf.truncate(len as usize / 2);
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                return Some(OsString::from_wide(&buf[..end]));
            }
            ERROR_MORE_DATA => buf.resize((len as usize / 2).max(buf.len() * 2), 0),
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directories_are_added_after_the_existing_ones() {
        let path = OsString::from(r"C:\Windows;C:\Users\me\bin\");
        let more = [
            OsString::from(r"c:\windows;C:\Tools"),
            OsString::from(r"C:\Users\me\bin;;C:\Tools\;C:\New"),
        ];
        assert_eq!(
            add_paths(&path, &more),
            OsString::from(r"C:\Windows;C:\Users\me\bin\;C:\Tools;C:\New")
        );
    }

    #[test]
    fn the_registry_path_is_read() {
        let path = read_path(
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        )
        .unwrap();
        assert!(!path.to_string_lossy().contains(['%', '\0']));
        assert!(std::env::split_paths(&path).any(|dir| dir.join("cmd.exe").is_file()));
    }
}
