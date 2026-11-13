//! What changed in an offered release: the `CHANGELOG.md` sections of the
//! versions after this copy's, up to the offered one, read from the
//! release's tag. The release workflow publishes the same sections as each
//! GitHub release's notes.

use anyhow::Result;
use semver::Version;

use super::release;

/// `CHANGELOG.md` at a release tag.
fn changelog_url(version: &Version) -> String {
    format!("https://raw.githubusercontent.com/raine/draftmux/v{version}/CHANGELOG.md")
}

/// The notes for updating from `installed` to `offered`, as Markdown;
/// `None` when the changelog has no section for those versions.
pub fn fetch(installed: &Version, offered: &Version) -> Result<Option<String>> {
    let changelog = release::fetch_text(&changelog_url(offered))?;
    Ok(select(&changelog, installed, offered))
}

/// The sections of `changelog` headed `## vX.Y.Z`, optionally followed by
/// a date, for the versions after `installed` up to `offered`, newest
/// first. Each keeps its heading one level down, so the versions read as
/// parts of one list of changes.
pub fn select(changelog: &str, installed: &Version, offered: &Version) -> Option<String> {
    let mut sections: Vec<(Version, String)> = Vec::new();
    let mut current: Option<(Version, String)> = None;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.extend(current.take());
            current = section_version(heading)
                .filter(|version| version > installed && version <= offered)
                .map(|version| (version, format!("### {}\n", heading.trim())));
        } else if let Some((_, text)) = &mut current {
            text.push_str(line);
            text.push('\n');
        }
    }
    sections.extend(current);
    sections.sort_by(|a, b| b.0.cmp(&a.0));
    let notes = sections
        .into_iter()
        .map(|(_, text)| text.trim().to_owned())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!notes.is_empty()).then_some(notes)
}

/// The version of a `## v1.2.3 (2026-11-12)` heading.
fn section_version(heading: &str) -> Option<Version> {
    let tag = heading.split_whitespace().next()?;
    Version::parse(tag.strip_prefix('v')?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "# Changelog

## Unreleased

- Not released yet.

## v0.3.0 (2026-12-01)

- Third.

  Continued paragraph.

## v0.2.0 (2026-11-20)

- Second.

## v0.1.0 (2026-11-12)

Initial release.
";

    fn v(raw: &str) -> Version {
        Version::parse(raw).unwrap()
    }

    #[test]
    fn selects_the_versions_after_the_installed_one() {
        assert_eq!(
            select(CHANGELOG, &v("0.1.0"), &v("0.3.0")).as_deref(),
            Some(
                "### v0.3.0 (2026-12-01)\n\n- Third.\n\n  Continued paragraph.\n\n\
                 ### v0.2.0 (2026-11-20)\n\n- Second."
            )
        );
    }

    #[test]
    fn stops_at_the_offered_version() {
        assert_eq!(
            select(CHANGELOG, &v("0.1.0"), &v("0.2.0")).as_deref(),
            Some("### v0.2.0 (2026-11-20)\n\n- Second.")
        );
    }

    #[test]
    fn leaves_out_unreleased_and_other_headings() {
        let notes = select(CHANGELOG, &v("0.0.1"), &v("9.0.0")).unwrap();
        assert!(!notes.contains("Not released yet"), "{notes}");
        assert!(notes.ends_with("### v0.1.0 (2026-11-12)\n\nInitial release."));
    }

    #[test]
    fn heading_without_a_date_is_a_section() {
        assert_eq!(
            select("## v1.0.0\n\n- Done.\n", &v("0.9.0"), &v("1.0.0")).as_deref(),
            Some("### v1.0.0\n\n- Done.")
        );
    }

    #[test]
    fn orders_sections_newest_first() {
        let changelog = "## v1.0.0\n\n- One.\n\n## v1.1.0\n\n- One one.\n";
        assert_eq!(
            select(changelog, &v("0.1.0"), &v("1.1.0")).as_deref(),
            Some("### v1.1.0\n\n- One one.\n\n### v1.0.0\n\n- One.")
        );
    }

    #[test]
    fn no_matching_section_means_no_notes() {
        assert_eq!(select(CHANGELOG, &v("0.3.0"), &v("0.4.0")), None);
        assert_eq!(select(CHANGELOG, &v("0.3.0"), &v("0.3.0")), None);
        assert_eq!(select("", &v("0.1.0"), &v("0.2.0")), None);
        assert_eq!(
            select("<!DOCTYPE html>\n404", &v("0.1.0"), &v("0.2.0")),
            None
        );
    }

    #[test]
    fn reads_the_repository_changelog() {
        let changelog = include_str!("../../../../CHANGELOG.md");
        let notes = select(changelog, &v("0.0.0"), &v("0.1.0")).unwrap();
        assert!(notes.starts_with("### v0.1.0"), "{notes}");
    }
}
