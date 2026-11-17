//! Automatic updates of a released Draftmux from the GitHub releases of
//! raine/draftmux. A Developer ID signed `Draftmux.app`, or a Linux app tree
//! from a release tarball, checks for a newer release when it starts and
//! then once a day, installs it in the background by swapping the app in
//! place and offers to restart into it. A Homebrew cask install updates
//! itself too: the cask declares `auto_updates`. A Windows release checks as
//! often and offers the download. Development builds never check on their
//! own; Check for Updates still reports a newer release to them. The toolbar
//! offers a newer release with what changed in it, and Settings shows where
//! updating stands.

mod cache;
#[cfg_attr(windows, path = "update/install_windows.rs")]
mod install;
mod notes;
mod release;

use std::time::{Duration, SystemTime};

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};
use semver::Version;

use crate::app_settings::AppSettings;
use install::Installation;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// `CFBundleIdentifier` in macos/Info.plist.
const BUNDLE_ID: &str = "fi.zendit.Draftmux";
/// Disables automatic checks when set to `1`, `true` or `yes`.
const NO_CHECK_VAR: &str = "DRAFTMUX_NO_UPDATE_CHECK";
/// How long after launch the first automatic check waits, and how often a
/// running app looks again whether one is due. The first check runs however
/// recently the last one did; later ones once a day.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const RECHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

gpui::actions!(draftmux, [CheckForUpdates]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release's GitHub page.
    pub page: String,
    pub archive_url: String,
    pub checksum_url: String,
    /// What changed since this copy's version, as Markdown; `None` when
    /// the changelog couldn't be read.
    pub notes: Option<String>,
}

/// Why this copy of Draftmux can't install updates itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ineligible {
    /// Not a Developer ID signed `Draftmux.app` or a Linux release tree:
    /// built from source.
    Development,
    /// The folder holding the app isn't writable by this user.
    #[cfg_attr(windows, allow(dead_code))]
    Unwritable(String),
    /// A release on a platform whose updates are installed by hand.
    #[cfg_attr(not(windows), allow(dead_code))]
    Manual,
}

/// The recipe that rebuilds and installs a development build.
const INSTALL_RECIPE: &str = if cfg!(target_os = "macos") {
    "just install-app"
} else if cfg!(windows) {
    "cargo build --release -p draftmux"
} else {
    "just linux-install"
};

impl Ineligible {
    fn guidance(&self, version: &Version) -> String {
        match self {
            Self::Development => format!(
                "Draftmux {version} is available. This is a development build: pull the \
                 repository and run {INSTALL_RECIPE}."
            ),
            Self::Unwritable(dir) => format!(
                "Draftmux {version} is available, but Draftmux can't replace itself in {dir}. \
                 Download it from GitHub."
            ),
            Self::Manual => format!("Draftmux {version} is available. Download it from GitHub."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking,
    /// The last check found no newer release.
    Current,
    /// A check asked for in Settings failed, with why.
    Failed(String),
    Installing(Version),
    /// The new version is installed and runs after a restart.
    Ready(Release),
    /// A newer release this copy can't install itself.
    Blocked {
        release: Release,
        reason: String,
    },
}

/// Where a check was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The first automatic check after launch.
    Launch,
    /// A later automatic check, which runs only when one is due.
    Scheduled,
    /// Check for Updates in the app menu: the main window reports the
    /// result.
    Menu,
    /// Check Now in Settings, which shows the result itself.
    Settings,
}

impl Origin {
    fn manual(self) -> bool {
        matches!(self, Self::Menu | Self::Settings)
    }
}

/// What the main window does for the updater.
pub enum Event {
    /// Show a message about a check from the menu.
    Notice { text: SharedString, error: bool },
    /// Open the toolbar's update card on the offered release.
    ShowOffer,
    /// Save and restart into the installed update.
    Restart,
}

pub struct Updater {
    pub status: Status,
    /// When a check last reached GitHub.
    pub checked_at: Option<SystemTime>,
    _poll: Task<()>,
}

struct GlobalUpdater(Entity<Updater>);

impl Global for GlobalUpdater {}

impl EventEmitter<Event> for Updater {}

pub fn init(cx: &mut App) {
    let entity = cx.new(|cx| Updater {
        status: dev_offer().unwrap_or(Status::Idle),
        checked_at: cache::last_checked(),
        _poll: cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            let mut origin = Origin::Launch;
            loop {
                let Ok(()) = this.update(cx, |this: &mut Updater, cx| this.check(origin, cx))
                else {
                    return;
                };
                origin = Origin::Scheduled;
                cx.background_executor().timer(RECHECK_INTERVAL).await;
            }
        }),
    });
    cx.set_global(GlobalUpdater(entity));
    cx.on_action(|_: &CheckForUpdates, cx| {
        Updater::global(cx).update(cx, |updater, cx| updater.check(Origin::Menu, cx));
    });
}

impl Updater {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalUpdater>().0.clone()
    }

    /// Looks for a newer release and installs it when this copy can. An
    /// automatic check runs only for an installable app and only while
    /// checks are on, and after the first one only when one is due; it
    /// reports nothing but its result. A check from the menu ends in a
    /// message or the update card; one from Settings only in `status`.
    pub fn check(&mut self, origin: Origin, cx: &mut Context<Self>) {
        let manual = origin.manual();
        match &self.status {
            Status::Idle | Status::Current | Status::Failed(_) | Status::Blocked { .. } => {}
            Status::Checking | Status::Installing(_) => return,
            Status::Ready(_) => {
                if origin == Origin::Menu {
                    cx.emit(Event::ShowOffer);
                }
                return;
            }
        }
        if !manual && !automatic_checks(cx) {
            return;
        }
        let previous = std::mem::replace(&mut self.status, Status::Checking);
        cx.notify();
        let task = cx.background_spawn(async move {
            let eligibility = install::installation();
            let built_here = matches!(eligibility, Err(Ineligible::Development));
            if !manual && (built_here || (origin == Origin::Scheduled && !cache::check_due())) {
                return Ok(None);
            }
            let release = cache::check()?.map(|mut release| {
                // Best effort: the update is offered without them.
                release.notes = notes::fetch(&current_version(), &release.version)
                    .ok()
                    .flatten();
                release
            });
            Ok::<_, anyhow::Error>(Some((release, eligibility)))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.status = previous;
                if let Ok(Some(_)) = &result {
                    this.checked_at = Some(SystemTime::now());
                }
                match result {
                    Ok(None) => {}
                    Ok(Some((None, _))) => {
                        this.status = Status::Current;
                        if origin == Origin::Menu {
                            cx.emit(notice(
                                format!("Draftmux {CURRENT_VERSION} is the latest version."),
                                false,
                            ));
                        }
                    }
                    Ok(Some((Some(release), Ok(installation)))) => {
                        this.install(release, installation, origin, cx);
                    }
                    Ok(Some((Some(release), Err(reason)))) => {
                        this.status = Status::Blocked {
                            reason: reason.guidance(&release.version),
                            release,
                        };
                        if origin == Origin::Menu {
                            cx.emit(Event::ShowOffer);
                        }
                    }
                    Err(err) => {
                        this.failed(format!("Couldn't check for updates: {err:#}"), origin, cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reports a failed check or install where it was asked for; an
    /// automatic one tries again later without a word.
    fn failed(&mut self, message: String, origin: Origin, cx: &mut Context<Self>) {
        match origin {
            Origin::Menu => cx.emit(notice(message, true)),
            Origin::Settings => self.status = Status::Failed(message),
            Origin::Launch | Origin::Scheduled => {}
        }
    }

    /// Asks the main window to save the drafts and restart into the
    /// installed update.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Ready(_)) {
            cx.emit(Event::Restart);
        }
    }

    fn install(
        &mut self,
        release: Release,
        installation: Installation,
        origin: Origin,
        cx: &mut Context<Self>,
    ) {
        let version = release.version.clone();
        let ready = release.clone();
        if origin == Origin::Menu {
            cx.emit(notice(format!("Downloading Draftmux {version}…"), false));
        }
        self.status = Status::Installing(version.clone());
        let task = cx.background_spawn(async move { install::install(&release, &installation) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        // gpui would restart the binary the kernel now
                        // reports as deleted; its path holds the new one.
                        #[cfg(target_os = "linux")]
                        if let Ok(exe) = crate::resources::exe() {
                            cx.set_restart_path(exe);
                        }
                        this.status = Status::Ready(ready);
                        if origin == Origin::Menu {
                            cx.emit(Event::ShowOffer);
                        }
                    }
                    Err(err) => {
                        this.status = Status::Idle;
                        this.failed(
                            format!("Couldn't install Draftmux {version}: {err:#}"),
                            origin,
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Debug builds only: an offer of the next minor version, for looking at
/// the update card without a newer release. `DRAFTMUX_DEV_UPDATE_OFFER` is
/// `ready` or `blocked`; the notes are selected from the changelog file at
/// `DRAFTMUX_DEV_UPDATE_CHANGELOG`, and fail to load without one.
fn dev_offer() -> Option<Status> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let kind = std::env::var("DRAFTMUX_DEV_UPDATE_OFFER").ok()?;
    let installed = current_version();
    let version = Version::new(installed.major, installed.minor + 1, 0);
    let notes = std::env::var_os("DRAFTMUX_DEV_UPDATE_CHANGELOG")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|changelog| notes::select(&changelog, &installed, &version));
    let release = Release {
        page: format!("https://github.com/raine/draftmux/releases/tag/v{version}"),
        archive_url: String::new(),
        checksum_url: String::new(),
        notes,
        version: version.clone(),
    };
    match kind.as_str() {
        "ready" => Some(Status::Ready(release)),
        "blocked" => Some(Status::Blocked {
            reason: Ineligible::Development.guidance(&version),
            release,
        }),
        _ => None,
    }
}

fn automatic_checks(cx: &App) -> bool {
    let setting = AppSettings::global(cx)
        .read(cx)
        .settings
        .check_for_updates
        .unwrap_or(true);
    setting && !env_disables(std::env::var(NO_CHECK_VAR).ok().as_deref())
}

fn env_disables(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        ["1", "true", "yes"]
            .iter()
            .any(|on| value.trim().eq_ignore_ascii_case(on))
    })
}

fn notice(text: String, error: bool) -> Event {
    Event::Notice {
        text: text.into(),
        error,
    }
}

fn current_version() -> Version {
    Version::parse(CURRENT_VERSION).expect("package version must be valid semver")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_id_matches_info_plist() {
        let plist = include_str!("../../../macos/Info.plist");
        let key = "<key>CFBundleIdentifier</key>";
        let value = plist[plist.find(key).unwrap() + key.len()..]
            .trim_start()
            .strip_prefix("<string>")
            .and_then(|rest| rest.split_once("</string>"))
            .map(|(id, _)| id);
        assert_eq!(value, Some(BUNDLE_ID));
    }

    #[test]
    fn env_var_disables_checks() {
        assert!(env_disables(Some("1")));
        assert!(env_disables(Some("YES")));
        assert!(!env_disables(Some("0")));
        assert!(!env_disables(Some("")));
        assert!(!env_disables(None));
    }
}
