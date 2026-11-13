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
//! insertions marked. A `<name>-review.txt` beside the images spells out
//! the marks, `[-n:…-]` deleted and `{+n:…+}` inserted for hunk `n`, with
//! `|` for a point.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use draftmux_storage::Target;
use gpui::{
    App, AsyncApp, Bounds, Context, Render, Window, WindowBounds, WindowOptions, div, point,
    prelude::*, px, size,
};
use markup::Document;

use crate::assets;
use crate::compare::{self, Mark, Run, Side};
use crate::inline_text::MarkHandlers;
use crate::markup_view::{self, MarkupStyle, Redline, RedlineColors};
use crate::theme::Palette;

/// Padding around the body in the image, matching the reference captures.
const PAD: f32 = 16.;

/// Render fixture drafts to PNGs (development tool).
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target whose preview style to use: github, slack or rich.
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
        "slack" => Target::Slack,
        "rich" => Target::Rich,
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
        let comparison = compare::compare(before, after);
        let sides = [Side::Before, Side::After]
            .map(|side| markup_view::redline_text(&comparison, side, target));
        let docs = sides
            .each_ref()
            .map(|text| markup_view::preview(target, text));
        let path = out.join(format!("{name}-review.txt"));
        std::fs::write(
            &path,
            format!(
                "{} hunks\n\n--- before\n{}\n--- after\n{}",
                comparison.hunks.len(),
                readable(&sides[0]),
                readable(&sides[1])
            ),
        )?;
        println!("{}", path.display());
        for dark in [false, true] {
            let palette = if dark {
                Palette::dark()
            } else {
                Palette::light()
            };
            let style = |doc: &Document| {
                let mut style = markup_view::style(target, dark);
                style.redline = Some(std::rc::Rc::new(Redline {
                    selected: None,
                    hovered: None,
                    kept: vec![false; comparison.hunks.len()],
                    colors: RedlineColors::new(&palette),
                    handlers: std::rc::Rc::new(MarkHandlers {
                        click: std::rc::Rc::new(|_, _, _| {}),
                        hover: std::rc::Rc::new(|_, _| {}),
                        placed: std::rc::Rc::new(|_, _| {}),
                    }),
                }));
                (doc.clone(), style)
            };
            let columns = [style(&docs[0]), style(&docs[1])];
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

/// Marked text with its marks spelled out: `[-n:…-]` deleted, `{+n:…+}`
/// inserted, `|` a point.
fn readable(marked: &str) -> String {
    let mut open = None;
    let mut out = String::new();
    let mut last: Option<Mark> = None;
    let close = |out: &mut String, mark: Mark| out.push_str(if mark.deleted { "-]" } else { "+}" });
    for (mark, run) in compare::split_marks(marked, &mut open) {
        if mark != last {
            if let Some(last) = last {
                close(&mut out, last);
            }
            if let Some(mark) = mark {
                let open = if mark.deleted { "[-" } else { "{+" };
                out.push_str(&format!("{open}{}:", mark.hunk));
            }
        }
        out.push_str(match run {
            Run::Text(text) => text,
            Run::Point => "|",
        });
        last = mark;
    }
    if let Some(last) = last {
        close(&mut out, last);
    }
    out
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

/// Review's before and after columns side by side.
struct ReviewCapture {
    columns: [(Document, MarkupStyle); 2],
    rule: gpui::Hsla,
    width: f32,
}

impl Render for ReviewCapture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let [(before, before_style), (after, after_style)] = &self.columns;
        let column = |doc: &Document, style: &MarkupStyle, id: &'static str| {
            div().p(px(PAD)).child(
                div()
                    .w(px(self.width))
                    .child(markup_view::render(doc, style, id, None)),
            )
        };
        // The rule only spans the columns, so the image crops below them.
        div().size_full().bg(before_style.background).child(
            div()
                .flex()
                .child(column(before, before_style, "before"))
                .child(
                    column(after, after_style, "after")
                        .border_l_1()
                        .border_color(self.rule),
                ),
        )
    }
}
