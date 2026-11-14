//! `draftmux <FILE>`: opens a file in Draftmux for editing, without an agent
//! session to hand off from.
//!
//! The draft starts with the file's text and is linked to it, so Save writes
//! it back. Its agent session starts empty, in the file's directory: on pi
//! when the settings choose pi, else on Claude Code when the app can run it,
//! else on pi. A file an active draft is already linked to focuses that
//! draft instead.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use draftmux_settings::{DraftBackend, Settings};
use draftmux_storage::{
    Backend, ClaudeSession, Draft, LinkedFile, NewDraft, PiSession, Store, Target,
};

use crate::{ipc, open, source};

/// Opens `file` in the app, creating its draft if needed.
pub fn main(file: &Path) -> Result<u8> {
    let home = std::env::home_dir().context("no home directory")?;
    let store = Store::open_default()?;
    let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let env = Env {
        home: &home,
        claude: &open::claude_available,
        claude_config_dir: claude_config_dir.as_deref(),
    };
    let draft = find_or_create(&store, &open::load_settings(), file, &env)?;
    match ipc::send(&ipc::Request::Focus {
        id: draft.id.clone(),
    }) {
        Ok(()) => Ok(0),
        Err(err) => {
            eprintln!(
                "draftmux: couldn't show {} in the app: {err:#}\n\
                 draftmux: run `draftmux focus {}` to try again",
                file.display(),
                draft.id
            );
            Ok(1)
        }
    }
}

/// What a new draft's agent depends on outside the settings.
struct Env<'a> {
    home: &'a Path,
    /// Fails unless the app can run Claude Code.
    claude: &'a dyn Fn() -> Result<(), String>,
    /// The caller's `CLAUDE_CONFIG_DIR`.
    claude_config_dir: Option<&'a Path>,
}

/// The active draft linked to `file`, else a new one. `settings` are
/// resolved for the file's directory and kept in a new draft.
fn find_or_create(store: &Store, settings: &Settings, file: &Path, env: &Env) -> Result<Draft> {
    let path =
        std::path::absolute(file).with_context(|| format!("resolving {}", file.display()))?;
    let scan = store.scan()?;
    scan.report_errors(&mut std::io::stderr())?;
    if let Some(draft) = scan.active().find(|draft| {
        draft
            .meta
            .file
            .as_ref()
            .is_some_and(|linked| linked.path == path)
    }) {
        return Ok(draft.clone());
    }
    create(store, settings, path, env)
}

fn create(store: &Store, settings: &Settings, path: PathBuf, env: &Env) -> Result<Draft> {
    let home = env.home;
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cwd = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?
        .to_owned();
    let resolved = settings.resolve(&cwd, home);
    let target = if open::is_repository_markdown(&path) {
        Target::GithubFile
    } else {
        resolved.target.value
    };
    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = Some(LinkedFile::new(&path, text.as_bytes()));
    let on_claude = resolved.backend.value == DraftBackend::SameAsSource && (env.claude)().is_ok();
    if on_claude {
        let mut draft_settings = resolved.draft_settings();
        // Pi's; the Claude session keeps its own.
        draft_settings.selection_model = None;
        let session = ClaudeSession {
            session_id: uuid::Uuid::new_v4(),
            transcript: None,
            // Unset is `~/.claude`, where a login kept in the keychain is
            // found.
            config_dir: env
                .claude_config_dir
                .filter(|dir| dir.is_absolute() && *dir != home.join(".claude"))
                .map(Path::to_path_buf),
            fork: None,
            model: resolved
                .claude_drafting_model
                .value
                .model()
                .map(str::to_owned),
            selection_model: resolved
                .claude_selection_model
                .value
                .model()
                .map(str::to_owned),
        };
        return store.create(NewDraft {
            target,
            title,
            source: None,
            cwd,
            backend: Backend::Claude(session),
            note: None,
            settings: draft_settings,
            text,
            file,
        });
    }
    let pi_agent_dir = source::settings_profile(resolved.profile.value.as_deref(), home);
    // Without an override the session names no model and pi uses the
    // profile's default.
    let model = match resolved.model_override() {
        Some(model) => open::model_selection(model),
        None => claude_convert::ModelSelection::default(),
    };
    let new = NewDraft {
        target,
        title,
        source: None,
        cwd: cwd.clone(),
        backend: Backend::pi(pi_agent_dir.clone()),
        note: None,
        settings: resolved.draft_settings(),
        text,
        file,
    };
    store.create_with_session(new, |draft| {
        let file = claude_convert::empty(&cwd, &model).write_into(&draft.session_dir())?;
        Ok(Backend::Pi(PiSession {
            agent_dir: pi_agent_dir,
            session_file: Some(file),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_claude() -> Result<(), String> {
        Err("no claude".into())
    }

    fn env<'a>(home: &'a Path, claude: &'a dyn Fn() -> Result<(), String>) -> Env<'a> {
        Env {
            home,
            claude,
            claude_config_dir: None,
        }
    }

    #[test]
    fn creates_a_linked_draft_then_finds_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("drafts"));
        let file = tmp.path().join("notes.md");
        std::fs::write(&file, "# Notes\n").unwrap();
        let settings = Settings::default();
        let env = env(tmp.path(), &no_claude);

        let draft = find_or_create(&store, &settings, &file, &env).unwrap();
        assert_eq!(store.read_text(&draft.id).unwrap(), "# Notes\n");
        let meta = &draft.meta;
        assert_eq!(meta.title, "notes.md");
        assert_eq!(meta.cwd, tmp.path());
        assert!(meta.source.is_none() && !meta.is_sample());
        assert!(!meta.first_draft_pending);
        assert_eq!(meta.file.as_ref().unwrap().path, file);
        // Without Claude Code, on pi.
        assert!(meta.pi().unwrap().session_file.as_ref().unwrap().is_file());

        let again = find_or_create(&store, &settings, &file, &env).unwrap();
        assert_eq!(again.id, draft.id);

        // An archived draft isn't reopened.
        store.archive(&draft.id).unwrap();
        let fresh = find_or_create(&store, &settings, &file, &env).unwrap();
        assert_ne!(fresh.id, draft.id);
    }

    #[test]
    fn runs_on_claude_code_when_available_unless_settings_choose_pi() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("drafts"));
        let file = tmp.path().join("notes.md");
        std::fs::write(&file, "# Notes\n").unwrap();
        let claude = || Ok(());
        let env = Env {
            claude_config_dir: Some(&tmp.path().join("claude")),
            ..env(tmp.path(), &claude)
        };

        let draft = find_or_create(&store, &Settings::default(), &file, &env).unwrap();
        let Backend::Claude(session) = &draft.meta.backend else {
            panic!("not on Claude Code: {:?}", draft.meta.backend);
        };
        assert_eq!(session.fork, None);
        assert_eq!(session.transcript, None);
        assert_eq!(session.config_dir, Some(tmp.path().join("claude")));
        assert_eq!(store.load(&draft.id).unwrap().meta, draft.meta);

        store.archive(&draft.id).unwrap();
        let mut settings = Settings::default();
        settings.defaults.backend = Some(DraftBackend::Pi);
        let draft = find_or_create(&store, &settings, &file, &env).unwrap();
        assert!(draft.meta.pi().is_some());
    }

    #[test]
    fn missing_file_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("drafts"));
        let file = tmp.path().join("missing.md");
        let env = env(tmp.path(), &no_claude);
        assert!(find_or_create(&store, &Settings::default(), &file, &env).is_err());
        assert!(store.scan().unwrap().drafts.is_empty());
    }
}
