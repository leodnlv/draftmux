use super::*;
use futures::channel::oneshot;
use gpui::TestAppContext;
use std::cell::Cell;
use std::rc::Rc;

fn setup(cx: &TestAppContext, text: &str) -> (Entity<Editor>, Entity<Formatter>) {
    cx.update(|cx| {
        let editor = cx.new(|cx| Editor::new(text, cx));
        let formatter = cx.new(|cx| Formatter::new(editor.clone(), cx));
        formatter.update(cx, |f, cx| f.set_draft_dir(PathBuf::from("/draft/a"), cx));
        (editor, formatter)
    })
}

fn pending(cx: &TestAppContext, formatter: &Entity<Formatter>, wrap: Wrap) -> oneshot::Sender<()> {
    let (send, receive) = oneshot::channel();
    cx.update(|cx| {
        formatter.update(cx, |f, cx| {
            f.format_with(
                move |input| async move {
                    receive.await.unwrap();
                    run_formatter(&input, wrap)
                },
                cx,
            );
        })
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| formatter.read(cx).running.is_some()));
    send
}

fn text(cx: &TestAppContext, editor: &Entity<Editor>) -> String {
    cx.read(|cx| editor.read(cx).text().to_owned())
}

fn message(cx: &TestAppContext, formatter: &Entity<Formatter>) -> Option<String> {
    cx.read(|cx| formatter.read(cx).message.as_ref().map(ToString::to_string))
}

#[gpui::test]
fn format_applies_once_with_undo_redo_and_events(cx: &mut TestAppContext) {
    let input = "* a\n* b\n";
    let (editor, formatter) = setup(cx, input);
    let edits = Rc::new(Cell::new(0));
    let _subscription = cx.update(|cx| {
        cx.subscribe(&editor, {
            let edits = edits.clone();
            move |_, event, _| {
                if *event == EditorEvent::Edited {
                    edits.set(edits.get() + 1);
                }
            }
        })
    });
    let release = pending(cx, &formatter, Wrap::Keep);
    // Pending work does not block editor selection or permit duplicate work.
    cx.update(|cx| {
        editor.update(cx, |e, cx| e.select_range(2..3, cx));
        formatter.update(cx, |f, cx| {
            f.format_with(|_| async { panic!("duplicate job") }, cx)
        });
    });
    assert_eq!(text(cx, &editor), input);
    release.send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(text(cx, &editor), "- a\n- b\n");
    assert_eq!(edits.get(), 1);
    assert!(cx.read(|cx| formatter.read(cx).available()));
    assert_eq!(message(cx, &formatter), None);
    cx.update(|cx| {
        assert!(editor.update(cx, |e, cx| e.undo(cx)));
    });
    assert_eq!(text(cx, &editor), input);
    cx.update(|cx| {
        assert!(!editor.update(cx, |e, cx| e.undo(cx)));
    });
    cx.update(|cx| {
        assert!(editor.update(cx, |e, cx| e.redo(cx)));
    });
    assert_eq!(text(cx, &editor), "- a\n- b\n");
}

#[gpui::test]
fn no_op_preserves_history_and_declines_have_a_distinct_message(cx: &mut TestAppContext) {
    let (editor, formatter) = setup(cx, "- a\n");
    cx.update(|cx| {
        editor.update(cx, |e, cx| e.replace_all("- b\n", cx));
        editor.update(cx, |e, cx| {
            e.undo(cx);
        });
    });
    cx.run_until_parked();
    cx.update(|cx| formatter.update(cx, |f, cx| f.format(Wrap::Keep, cx)));
    cx.run_until_parked();
    assert_eq!(
        message(cx, &formatter).as_deref(),
        Some("Already formatted")
    );
    cx.update(|cx| {
        assert!(editor.update(cx, |e, cx| e.redo(cx)));
    });
    assert_eq!(text(cx, &editor), "- b\n");
    assert_eq!(message(cx, &formatter), None);
    cx.update(|cx| {
        editor.update(cx, |e, cx| e.replace_all("---\nunfinished", cx));
    });
    cx.run_until_parked();
    cx.update(|cx| formatter.update(cx, |f, cx| f.format(Wrap::Keep, cx)));
    cx.run_until_parked();
    assert_eq!(text(cx, &editor), "---\nunfinished");
    assert!(message(cx, &formatter).unwrap().contains("safety"));
}

#[gpui::test]
fn every_owner_or_text_transition_invalidates_pending_work(cx: &mut TestAppContext) {
    for scenario in 0..9 {
        let (editor, formatter) = setup(cx, "* a\n");
        let release = pending(cx, &formatter, Wrap::Keep);
        cx.update(|cx| match scenario {
            0 => editor.update(cx, |e, cx| e.replace_all("new text", cx)),
            1 => {
                editor.update(cx, |e, cx| e.replace_all("new text", cx));
                editor.update(cx, |e, cx| {
                    e.undo(cx);
                });
            }
            2 | 3 => formatter.update(cx, |f, cx| {
                f.set_draft_dir("/draft/b".into(), cx);
                if scenario == 3 {
                    f.set_draft_dir("/draft/a".into(), cx);
                }
            }),
            // Both GitHub and Rich call set_hidden(false) on target changes.
            4 => formatter.update(cx, |f, cx| f.set_hidden(false, cx)),
            5 | 6 => formatter.update(cx, |f, cx| {
                f.set_hidden(true, cx);
                if scenario == 6 {
                    f.set_hidden(false, cx);
                }
            }),
            7 | 8 => formatter.update(cx, |f, cx| {
                f.set_agent_running(true, cx);
                if scenario == 8 {
                    f.set_agent_running(false, cx);
                }
            }),
            _ => unreachable!(),
        });
        cx.run_until_parked();
        let before = text(cx, &editor);
        release.send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(text(cx, &editor), before, "scenario {scenario}");
        assert_eq!(message(cx, &formatter), None, "scenario {scenario}");
        assert!(cx.read(|cx| formatter.read(cx).running.is_none()));
        // A discarded completion leaves no extra undo step.
        if scenario != 0 {
            cx.update(|cx| {
                assert!(
                    !editor.update(cx, |e, cx| e.undo(cx)),
                    "scenario {scenario}"
                );
            });
        }
        cx.update(|cx| {
            formatter.update(cx, |f, cx| {
                f.set_hidden(false, cx);
                f.set_agent_running(false, cx);
                f.format(Wrap::Keep, cx);
            })
        });
        cx.run_until_parked();
        assert!(cx.read(|cx| formatter.read(cx).available()));
    }
}

#[gpui::test]
fn availability_closes_menus_and_blocks_queued_actions(cx: &mut TestAppContext) {
    for scenario in 0..3 {
        let (editor, formatter) = setup(cx, "* a\n");
        cx.update(|cx| {
            formatter.update(cx, |f, cx| {
                f.menu_open = true;
                match scenario {
                    0 => f.set_hidden(true, cx),
                    1 => f.set_agent_running(true, cx),
                    _ => {
                        f.draft_dir = None;
                        f.menu_open = false;
                    }
                }
                assert!(!f.available());
                assert!(!f.menu_open);
                f.format(Wrap::Always, cx);
                f.format(Wrap::Never, cx);
                assert!(f.running.is_none());
                f.set_hidden(false, cx);
                assert!(!f.menu_open);
            })
        });
        cx.run_until_parked();
        assert_eq!(text(cx, &editor), "* a\n");
    }
}

#[gpui::test]
fn errors_clear_running_and_windows_are_independent(cx: &mut TestAppContext) {
    let (a, fa) = setup(cx, "* a\n");
    let (b, fb) = setup(cx, "* b\n");
    let release = pending(cx, &fa, Wrap::Keep);
    cx.update(|cx| {
        fb.update(cx, |f, cx| {
            f.format_with(
                |_| async { recover_format(|| panic!("injected formatter panic")) },
                cx,
            )
        })
    });
    cx.run_until_parked();
    assert_eq!(text(cx, &b), "* b\n");
    assert!(cx.read(|cx| fb.read(cx).available()));
    cx.update(|cx| fb.update(cx, |f, cx| f.format(Wrap::Keep, cx)));
    cx.run_until_parked();
    assert_eq!(text(cx, &b), "- b\n");
    assert_eq!(text(cx, &a), "* a\n");
    release.send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(text(cx, &a), "- a\n");
}

#[gpui::test]
fn closing_owner_cancels_pending_completion(cx: &mut TestAppContext) {
    let (editor, formatter) = setup(cx, "* a\n");
    let release = pending(cx, &formatter, Wrap::Keep);
    let weak = formatter.downgrade();
    drop(formatter);
    cx.run_until_parked();
    let _ = release.send(());
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    assert_eq!(text(cx, &editor), "* a\n");
}

#[gpui::test]
fn real_background_formatter_runs_all_modes_without_shell_environment(cx: &mut TestAppContext) {
    for (wrap, input, expected) in [
        (Wrap::Keep, "* a\n* b\n".to_owned(), "- a\n- b\n".to_owned()),
        (Wrap::Never, "a\nb\n".to_owned(), "a b\n".to_owned()),
        (
            Wrap::Always,
            format!("{}\n", vec!["word"; 17].join(" ")),
            format!("{}\nword\n", vec!["word"; 16].join(" ")),
        ),
    ] {
        let (editor, formatter) = setup(cx, &input);
        cx.update(|cx| formatter.update(cx, |f, cx| f.format(wrap, cx)));
        cx.run_until_parked();
        assert_eq!(text(cx, &editor), expected);
    }
}
