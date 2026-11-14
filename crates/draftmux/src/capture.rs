//! `draftmux capture`: renders fixture drafts to PNGs for comparing the
//! preview with GitHub's, Slack's and Apple Mail's own rendering. For Rich
//! text it also writes the HTML Copy puts on the pasteboard, which
//! `tools/mail-reference` renders the way Apple Mail does.
//!
//! Each fixture is drawn by [`markup_view::render`] in an offscreen window,
//! light and dark, at a fixed content width on the platform's background,
//! and read back with gpui's `Window::render_to_image`. It leaves out the
//! preview's footnotes on what Slack posts differently, since Slack never
//! shows them. Built only with the `capture` feature, which needs gpui's test
//! support.
//!
//! With `--review` the fixtures are pairs, `<name>.before.md` and
//! `<name>.after.md`, and each is drawn as review's two columns: the
//! before text with its deletions marked beside the after text with its
//! insertions marked, each under its frontmatter's fields. A
//! `<name>-review.txt` beside the images spells out the marks on each text
//! drawn, `[-n:…-]` deleted and `{+n:…+}` inserted for hunk `n`, and
//! `<n|>` for a point.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use draftmux_core::decorations::{Side, decorations};
use draftmux_core::diff::compare;
use draftmux_storage::Target;
use gpui::{
    App, AsyncApp, Bounds, Context, Render, Window, WindowBounds, WindowOptions, div, point,
    prelude::*, px, size,
};
use markup::Document;

use crate::assets;
use crate::inline_text::{MarkHandlers, Segment};
use crate::markup_view::{self, Marks, MarkupStyle, Redline, RedlineColors};
use crate::theme::Palette;

/// Padding around the body in the image, matching the reference captures.
const PAD: f32 = 16.;

/// Render fixture drafts to PNGs (development tool).
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target whose preview style to use: github, github-file, slack, rich or
    /// plain.
    #[arg(long)]
    target: String,
    /// Directory of `.md` fixture drafts.
    fixtures: PathBuf,
    /// Directory to write `<fixture>-<target>-<light|dark>-draftmux.png` to.
    out: PathBuf,
    /// Content width in points; of each column with `--review`.
    #[arg(long, default_value_t = 640.)]
    width: f32,
    /// Draw `<name>.before.md` and `<name>.after.md` pairs as review's
    /// before and after columns.
    #[arg(long)]
    review: bool,
}

pub fn main(args: Args) -> u8 {
    let target = match args.target.as_str() {
        "github" => Target::Github,
        "github-file" => Target::GithubFile,
        "slack" => Target::Slack,
        "rich" => Target::Rich,
        "plain" => Target::Plain,
        other => {
            eprintln!("draftmux capture: unknown target {other}");
            return 2;
        }
    };
    let fixtures = match fixtures(&args.fixtures) {
        Ok(fixtures) => fixtures,
        Err(err) => {
            eprintln!("draftmux capture: {err:#}");
            return 1;
        }
    };
    std::fs::create_dir_all(&args.out).expect("create the output directory");
    let review = args.review;
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            markup_view::init(cx);
            cx.spawn(async move |cx| {
                let result = if review {
                    capture_reviews(target, &args.target, &fixtures, &args.out, args.width, cx)
                        .await
                } else {
                    capture_all(target, &args.target, &fixtures, &args.out, args.width, cx).await
                };
                let code = match result {
                    Ok(()) => 0,
                    Err(err) => {
                        eprintln!("draftmux capture: {err:#}");
                        1
                    }
                };
                cx.update(|cx| cx.quit());
                std::process::exit(code);
            })
            .detach();
        });
    0
}

fn fixtures(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&path)?));
        }
    }
    out.sort();
    Ok(out)
}

async fn capture_all(
    target: Target,
    target_name: &str,
    fixtures: &[(String, String)],
    out: &Path,
    width: f32,
    cx: &mut AsyncApp,
) -> Result<()> {
    for (name, text) in fixtures {
        if target == Target::Rich {
            let path = out.join(format!("{name}-{target_name}.html"));
            std::fs::write(&path, markup::rich_text::html(&markup::github::parse(text)))?;
            println!("{}", path.display());
        }
        for dark in [false, true] {
            let doc = markup_view::preview(target, text);
            let style = markup_view::style(target, dark);
            let image = snapshot(width + 2. * PAD, cx, move |cx| {
                cx.new(|_| Capture { doc, style, width })
            })
            .await?;
            let mode = if dark { "dark" } else { "light" };
            let path = out.join(format!("{name}-{target_name}-{mode}-draftmux.png"));
            image.save(&path)?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// Pairs `<name>.before.md` with `<name>.after.md` and draws each pair as
/// review's two columns.
async fn capture_reviews(
    target: Target,
    target_name: &str,
    fixtures: &[(String, String)],
    out: &Path,
    width: f32,
    cx: &mut AsyncApp,
) -> Result<()> {
    let text = |name: &str| {
        fixtures
            .iter()
            .find(|(fixture, _)| fixture == name)
            .map(|(_, text)| text.as_str())
    };
    let names = fixtures
        .iter()
        .filter_map(|(name, _)| name.strip_suffix(".before"));
    for name in names {
        let before = text(&format!("{name}.before")).unwrap_or_default();
        let Some(after) = text(&format!("{name}.after")) else {
            bail!("{name}.before.md has no {name}.after.md");
        };
        let comparison = compare(before, after);
        let decorations = decorations(&comparison);
        let sides = [(Side::Before, before), (Side::After, after)].map(|(side, text)| {
            let mapped = markup_view::preview_mapped(target, text);
            let redlined = markup_view::redline(&mapped, &decorations, side);
            (side, mapped.fields, redlined)
        });
        let redline = |marks: &std::rc::Rc<Marks>, palette: &Palette| Redline {
            marks: marks.clone(),
            selected: None,
            hovered: None,
            kept: vec![false; comparison.hunks.len()],
            colors: RedlineColors::new(palette),
            handlers: std::rc::Rc::new(MarkHandlers {
                click: std::rc::Rc::new(|_, _, _| {}),
                hover: std::rc::Rc::new(|_, _| {}),
                placed: std::rc::Rc::new(|_, _| {}),
            }),
        };
        let spelled = sides.each_ref().map(|(side, fields, redlined)| {
            let mut style = markup_view::style(target, false);
            style.redline = Some(std::rc::Rc::new(redline(
                &redlined.marks,
                &Palette::light(),
            )));
            readable(*side, fields, &redlined.document, &style)
        });
        let path = out.join(format!("{name}-review.txt"));
        std::fs::write(
            &path,
            format!(
                "{} hunks\n\n--- before\n{}\n--- after\n{}",
                comparison.hunks.len(),
                spelled[0],
                spelled[1]
            ),
        )?;
        println!("{}", path.display());
        for dark in [false, true] {
            let palette = if dark {
                Palette::dark()
            } else {
                Palette::light()
            };
            let columns = sides.each_ref().map(|(_, fields, redlined)| {
                let mut style = markup_view::style(target, dark);
                let line = std::rc::Rc::new(redline(&redlined.marks, &palette));
                style.redline = Some(line.clone());
                Column {
                    doc: (*redlined.document).clone(),
                    style,
                    fields: fields.clone(),
                    redline: line,
                    muted: palette.muted,
                }
            });
            let rule = palette.hairline;
            let image = snapshot(2. * (width + 2. * PAD) + 1., cx, move |cx| {
                cx.new(|_| ReviewCapture {
                    columns,
                    rule,
                    width,
                })
            })
            .await?;
            let mode = if dark { "dark" } else { "light" };
            let path = out.join(format!("{name}-{target_name}-{mode}-review.png"));
            image.save(&path)?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// What one side of a review draws, with its marks spelled out: each
/// changed field as `key: value`, then each text drawn, one per line.
fn readable(
    side: Side,
    fields: &markup::frontmatter::Fields,
    doc: &Document,
    style: &MarkupStyle,
) -> String {
    let mut out = String::new();
    for (ix, (key, value)) in fields.iter().enumerate() {
        let segments = markup_view::field_segments(value, ix, style.redline.as_ref().unwrap());
        out.push_str(&format!("{key}: {}\n", spell(side, &segments)));
    }
    for (path, segments) in markup_view::drawn(doc, style) {
        out.push_str(&format!("{path:?} {}\n", spell(side, &segments)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn frontmatter_transcript_matches_rendered_segments() {
        let before = include_str!("../fixtures/review/frontmatter-multiple.before.md");
        let after = include_str!("../fixtures/review/frontmatter-multiple.after.md");
        let comparison = compare(before, after);
        assert_eq!(comparison.hunks.len(), 2);
        let decorations = decorations(&comparison);
        for (side, text) in [(Side::Before, before), (Side::After, after)] {
            let mapped = markup_view::preview_mapped(Target::Github, text);
            let redlined = markup_view::redline(&mapped, &decorations, side);
            let mut style = markup_view::style(Target::Github, false);
            style.redline = Some(Rc::new(Redline {
                marks: redlined.marks.clone(),
                selected: None,
                hovered: None,
                kept: vec![false; comparison.hunks.len()],
                colors: RedlineColors::new(&Palette::light()),
                handlers: Rc::new(MarkHandlers {
                    click: Rc::new(|_, _, _| {}),
                    hover: Rc::new(|_, _| {}),
                    placed: Rc::new(|_, _| {}),
                }),
            }));
            let expected = format!(
                "subject: {}\n",
                spell(
                    side,
                    &markup_view::field_segments(
                        &mapped.fields["subject"],
                        0,
                        style.redline.as_ref().unwrap()
                    )
                )
            );
            let transcript = readable(side, &mapped.fields, &redlined.document, &style);
            assert!(
                transcript.starts_with(&expected),
                "expected {expected:?}, got {transcript:?}"
            );
        }
    }
}

/// `segments` with their marks spelled out. A mark without a background is
/// a point.
fn spell(side: Side, segments: &[Segment]) -> String {
    let deleted = side == Side::Before;
    let close = if deleted { "-]" } else { "+}" };
    let mut out = String::new();
    let mut open: Option<usize> = None;
    for segment in segments {
        let mark = segment.mark.as_ref();
        let point = mark.filter(|mark| mark.background.is_none());
        let now = mark.filter(|_| point.is_none()).map(|mark| mark.hunk);
        if now != open {
            if open.is_some() {
                out.push_str(close);
            }
            if let Some(hunk) = now {
                out.push_str(&format!("{}{hunk}:", if deleted { "[-" } else { "{+" }));
            }
            open = now;
        }
        match point {
            Some(point) => out.push_str(&format!("<{}|>", point.hunk)),
            None => out.push_str(&segment.text),
        }
    }
    if open.is_some() {
        out.push_str(close);
    }
    out.replace('\n', "\\n")
}

/// Draws the view `build` makes in an offscreen window `width` wide and
/// reads it back, cropped below its content.
async fn snapshot<V: Render>(
    width: f32,
    cx: &mut AsyncApp,
    build: impl FnOnce(&mut App) -> gpui::Entity<V> + 'static,
) -> Result<image::RgbaImage> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(-10000.), px(-10000.)),
                    size: size(px(width), px(2400.)),
                })),
                focus: false,
                show: true,
                ..Default::default()
            },
            |_, cx| build(cx),
        )
    })?;
    // Two frames: the first lays out, the second paints the result.
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    let image = window.update(cx, |_, window, _| {
        if window.scale_factor() != 2. {
            bail!(
                "scale factor is {}, references are 2x",
                window.scale_factor()
            );
        }
        window.render_to_image()
    })??;
    window.update(cx, |_, window, _| window.remove_window())?;
    Ok(crop(image))
}

/// Cuts the image below the last row that isn't background, keeping the
/// bottom padding.
fn crop(image: image::RgbaImage) -> image::RgbaImage {
    let background = *image.get_pixel(0, 0);
    let last = (0..image.height())
        .rev()
        .find(|&y| (0..image.width()).any(|x| *image.get_pixel(x, y) != background))
        .unwrap_or(0);
    let height = (last + 1 + 2 * PAD as u32).min(image.height());
    image::imageops::crop_imm(&image, 0, 0, image.width(), height).to_image()
}

struct Capture {
    doc: Document,
    style: MarkupStyle,
    width: f32,
}

impl Render for Capture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(self.style.background)
            .p(px(PAD))
            .child(div().w(px(self.width)).child(markup_view::render(
                &self.doc,
                &self.style,
                "capture",
                None,
            )))
    }
}

/// One of review's columns: its fields over its redline.
struct Column {
    doc: Document,
    style: MarkupStyle,
    fields: markup::frontmatter::Fields,
    redline: std::rc::Rc<Redline>,
    muted: gpui::Hsla,
}

/// Review's before and after columns side by side.
struct ReviewCapture {
    columns: [Column; 2],
    rule: gpui::Hsla,
    width: f32,
}

impl Render for ReviewCapture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let [before, after] = &self.columns;
        let column = |column: &Column, id: &'static str| {
            let fields = column.fields.iter().enumerate().map(|(ix, (key, value))| {
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .child(div().w(px(64.)).text_color(column.muted).child(key.clone()))
                    .child(markup_view::field_value(value, ix, &column.redline))
            });
            div().p(px(PAD)).child(
                div()
                    .w(px(self.width))
                    .font_family(column.style.font_family.clone())
                    .text_color(column.style.text_color)
                    .children(fields)
                    .when(!column.fields.is_empty(), |this| {
                        this.child(div().h(px(PAD)))
                    })
                    .child(markup_view::render(&column.doc, &column.style, id, None)),
            )
        };
        // The rule only spans the columns, so the image crops below them.
        div().size_full().bg(before.style.background).child(
            div()
                .flex()
                .child(column(before, "before"))
                .child(column(after, "after").border_l_1().border_color(self.rule)),
        )
    }
}
