//! Reproducible CPU benchmarks for the synchronous typing pipeline.
//!
//! Run with `cargo test -j 2 -p draftmux --bin draftmux performance::typing
//! -- --ignored --nocapture`, or `performance::review_typing` for the review
//! comparison. These measure CPU work, not input-to-display latency.

#[path = "../../editor/src/highlight.rs"]
#[allow(dead_code)]
mod highlight;

use std::collections::HashSet;
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

use crate::compare::{self, Side};

fn measure(name: &str, mut work: impl FnMut()) -> (f64, f64) {
    for _ in 0..5 {
        work();
    }
    let mut samples = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        work();
        samples.push(start.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "{name}: median={:.3}ms p95={:.3}ms",
        samples[25], samples[47]
    );
    (samples[25], samples[47])
}

#[test]
#[ignore = "manual CPU benchmark"]
fn typing() {
    let section = "## Release readiness\n\nWe reviewed the **deployment plan** with the platform team. The next step is to validate *rollback safety* and [service health](https://example.com/health) before enabling traffic.\n\n- [x] Verify the staging configuration\n- [ ] Confirm ownership and update the runbook\n\n> Keep the previous version available until validation completes.\n\n```rust\nfn ready(count: usize) -> bool { count > 0 }\n```\n\n| Service | Status |\n| --- | --- |\n| API | Ready |\n| Worker | Pending |\n\n";
    for (name, repeats) in [("small", 2), ("large", 200)] {
        let mut text = section.repeat(repeats);
        text.push_str("A final update");
        eprintln!("{name}: {} bytes", text.len());
        measure(&format!("{name}/highlight"), || {
            black_box(highlight::highlight(black_box(&text)));
        });
        for target in [
            draftmux_storage::Target::Github,
            draftmux_storage::Target::Slack,
            draftmux_storage::Target::Rich,
        ] {
            measure(&format!("{name}/{target:?}/parse"), || {
                black_box(crate::markup_view::preview(target, black_box(&text)));
            });
            let doc = crate::markup_view::preview(target, &text);
            let style = crate::markup_view::style(target, false);
            measure(&format!("{name}/{target:?}/element-tree"), || {
                black_box(crate::markup_view::render(&doc, &style, "benchmark", None));
            });
        }
        measure(&format!("{name}/copies"), || {
            black_box((text.clone(), text.clone()));
        });
    }
}

const WORDS: &[&str] = &[
    "agent", "draft", "review", "change", "latency", "budget", "frame", "editor", "preview",
    "release", "service", "rollback", "traffic", "owner", "runbook", "staging", "metric", "signal",
    "deploy", "config", "health", "worker", "queue", "retry", "timeout", "cache", "thread",
    "window", "buffer", "layout", "render", "parser", "token", "budget", "summary", "follow",
    "update", "ticket", "launch", "region", "replica", "schema", "storage", "index", "request",
    "handler", "client", "server", "plan", "check", "verify", "confirm", "enable", "disable",
    "measure", "compare", "estimate", "migrate", "monitor", "alert",
];

/// A deterministic word source, so every run benchmarks the same drafts.
struct Words(u64);

impl Words {
    fn next(&mut self) -> &'static str {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        WORDS[(self.0 >> 33) as usize % WORDS.len()]
    }

    fn sentence(&mut self, count: usize) -> String {
        let words: Vec<&str> = (0..count).map(|_| self.next()).collect();
        let mut sentence = words.join(" ");
        sentence[..1].make_ascii_uppercase();
        sentence.push('.');
        sentence
    }

    fn paragraph(&mut self) -> String {
        (0..4)
            .map(|k| self.sentence(8 + k * 3))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Markdown of about `size` bytes: headed sections of one-line paragraphs
/// and lists, with a code block in the first section and every third after.
fn draft(size: usize) -> String {
    let mut words = Words(7);
    let mut out = String::new();
    for section in 1.. {
        let mut blocks = vec![
            format!("## {section}. {}\n\n", words.sentence(4)),
            format!("{}\n\n", words.paragraph()),
            (0..3)
                .map(|_| format!("- {}\n", words.sentence(7)))
                .collect::<String>()
                + "\n",
        ];
        if section % 3 == 1 {
            blocks.push(format!(
                "```rust\nfn {}(count: usize) -> bool {{\n    count > {section} // {}\n}}\n```\n\n",
                words.next(),
                words.sentence(5)
            ));
        }
        blocks.push(format!("{}\n\n", words.paragraph()));
        for block in blocks {
            if out.len() >= size {
                return out;
            }
            out.push_str(&block);
        }
    }
    unreachable!()
}

/// The draft before an agent edit that changed it in five places: reworded
/// a heading and two paragraphs, added a paragraph and removed a list item.
fn lightly_edited(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut words = Words(11);
    let len = lines.len();
    let nearest = |lines: &[String], at: usize, wanted: &dyn Fn(&str) -> bool| {
        (at..lines.len())
            .chain(0..at)
            .find(|&k| wanted(&lines[k]))
            .expect("the draft has every kind of line")
    };
    let prose = |line: &str| line.split(' ').count() > 12 && !line.starts_with(['-', '#', ' ']);
    // Furthest first, so earlier indices stay put.
    let at = nearest(&lines, len * 93 / 100, &prose);
    let mut reworded: Vec<&str> = lines[at].split(' ').collect();
    reworded.splice(5..9, ["and", "then", "after", "that"]);
    lines[at] = reworded.join(" ");
    let at = nearest(&lines, len * 80 / 100, &|line| line.starts_with("- "));
    lines.insert(at, format!("- {}", words.sentence(7)));
    let at = nearest(&lines, len * 62 / 100, &prose);
    lines.remove(at);
    let at = nearest(&lines, len * 27 / 100, &prose);
    let mut reworded: Vec<&str> = lines[at].split(' ').collect();
    reworded[3] = "previously";
    reworded[10] = "nothing";
    lines[at] = reworded.join(" ");
    let at = nearest(&lines, len * 8 / 100, &|line| line.starts_with("## "));
    lines[at] = format!("## {}", words.sentence(3));
    lines.join("\n") + "\n"
}

/// The draft before an agent rewrote most of it: four in five lines of
/// text have reworded words, and one in five is replaced outright.
fn heavily_rewritten(text: &str) -> String {
    let mut words = Words(13);
    let mut out = String::new();
    for (k, line) in text.lines().enumerate() {
        let plain = !line.is_empty() && !line.starts_with(['`', ' ', '}', 'f']);
        if plain && k % 5 == 0 {
            out.push_str(&words.sentence(line.split(' ').count()));
        } else if plain && k % 5 != 4 {
            let reworded: Vec<&str> = line
                .split(' ')
                .enumerate()
                .map(|(at, word)| match at > 1 && at % 3 == 0 {
                    true => words.next(),
                    false => word,
                })
                .collect();
            out.push_str(&reworded.join(" "));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Where typing goes: the end, the middle of an unchanged paragraph's
/// line, and inside the change nearest the middle.
fn typing_positions(base: &str, text: &str) -> [(&'static str, usize); 3] {
    let middle = text[text.len() / 2..]
        .find("\n")
        .map_or(text.len(), |at| text.len() / 2 + at);
    let comparison = compare::compare(base, text);
    let change = comparison
        .hunks
        .iter()
        .filter(|hunk| hunk.added.trim().len() > 2)
        .min_by_key(|hunk| hunk.new.start.abs_diff(text.len() / 2))
        .expect("the agent added text");
    [
        ("end", text.len()),
        ("middle", middle),
        ("in-change", change.new.start + 1),
    ]
}

/// What `Chrome::sync_review` does on each edit while the review is closed:
/// copy the editor's text, find the cached comparison stale, compare again,
/// keep both texts for the next check, and count the unkept changes for the
/// Changes button.
fn sync_closed(
    base: &str,
    text: &str,
    cached: &mut Option<Rc<(String, String, compare::Comparison)>>,
    kept: &HashSet<String>,
) -> usize {
    let text = text.to_owned();
    if let Some(cached) = cached
        && cached.0 == base
        && cached.1 == text
    {
        return 0;
    }
    let comparison = compare::compare(base, &text);
    let compared = Rc::new((base.to_owned(), text, comparison));
    let unreviewed = compared
        .2
        .hunks
        .iter()
        .filter(|hunk| {
            // `Changes::key` for the first edit.
            let key = format!("0\0{}\0{}\0{}", hunk.old.start, hunk.removed, hunk.added);
            !kept.contains(&key)
        })
        .count();
    *cached = Some(compared);
    unreviewed
}

#[test]
#[ignore = "manual CPU benchmark"]
fn review_typing() {
    let target = draftmux_storage::Target::Github;
    let palette = crate::theme::Palette::light();
    let mut rows = Vec::new();
    for (size_name, size) in [
        ("1K", 1_000),
        ("3K", 3_000),
        ("10K", 10_000),
        ("30K", 30_000),
    ] {
        let text = draft(size);
        for (base_name, base) in [
            ("light", lightly_edited(&text)),
            ("heavy", heavily_rewritten(&text)),
        ] {
            let hunks = compare::compare(&base, &text).hunks.len();
            eprintln!(
                "{size_name}/{base_name}: {} bytes, base {} bytes, {hunks} changes",
                text.len(),
                base.len()
            );
            for (place, at) in typing_positions(&base, &text) {
                let name = format!("{size_name}/{base_name}/{place}");
                let typed = "then quickly ".chars().cycle();
                // Each sample is one keystroke, so the cache never hits.
                let mut typing = (text.clone(), at, typed.clone());
                let mut cached = None;
                let kept = HashSet::new();
                let closed = measure(&format!("{name}/closed"), || {
                    let (text, at, typed) = &mut typing;
                    text.insert(*at, typed.next().unwrap());
                    *at += 1;
                    black_box(sync_closed(&base, text, &mut cached, &kept));
                });
                // What the open review adds on each keystroke once the
                // comparison is done: both columns' redlines, as `marked`
                // alone and with the parse, and their element trees.
                let comparison = compare::compare(&base, &text);
                let marked = measure(&format!("{name}/marked"), || {
                    for side in [Side::Before, Side::After] {
                        black_box(compare::marked(&comparison, side, &[]));
                    }
                });
                let redline = measure(&format!("{name}/redline"), || {
                    for side in [Side::Before, Side::After] {
                        black_box(crate::markup_view::redline(&comparison, side, target));
                    }
                });
                let documents = [Side::Before, Side::After]
                    .map(|side| crate::markup_view::redline(&comparison, side, target));
                let mut style = crate::markup_view::style(target, false);
                style.redline = Some(Rc::new(crate::markup_view::Redline {
                    selected: None,
                    hovered: None,
                    kept: vec![false; comparison.hunks.len()],
                    colors: crate::markup_view::RedlineColors::new(&palette),
                    handlers: Rc::new(crate::inline_text::MarkHandlers {
                        click: Rc::new(|_, _, _| {}),
                        hover: Rc::new(|_, _| {}),
                        placed: Rc::new(|_, _| {}),
                    }),
                }));
                let tree = measure(&format!("{name}/element-tree"), || {
                    for document in &documents {
                        black_box(crate::markup_view::render(
                            document,
                            &style,
                            "benchmark",
                            None,
                        ));
                    }
                });
                let mut typing = (text.clone(), at, typed);
                let mut cached = None;
                let open = measure(&format!("{name}/open"), || {
                    let (text, at, typed) = &mut typing;
                    text.insert(*at, typed.next().unwrap());
                    *at += 1;
                    black_box(sync_closed(&base, text, &mut cached, &kept));
                    let comparison = &cached.as_ref().unwrap().2;
                    for side in [Side::Before, Side::After] {
                        let document = crate::markup_view::redline(comparison, side, target);
                        black_box(crate::markup_view::render(
                            &document,
                            &style,
                            "benchmark",
                            None,
                        ));
                    }
                });
                rows.push((name, closed, marked, redline, tree, open));
            }
        }
    }
    eprintln!(
        "\n| Draft | Closed: compare + count | Open: marked | Open: redline docs | Open: element trees | Open: total |"
    );
    eprintln!("| --- | --- | --- | --- | --- | --- |");
    for (name, closed, marked, redline, tree, open) in rows {
        let cell = |(median, p95): (f64, f64)| format!("{median:.2} / {p95:.2}");
        eprintln!(
            "| {name} | {} | {} | {} | {} | {} |",
            cell(closed),
            cell(marked),
            cell(redline),
            cell(tree),
            cell(open)
        );
    }
}
