//! Real-root ports of assistant.e2e.ts. Fake CLIs still use the production MCP server.
mod ui_c_support;
use gpui_kit::{point, px, AppContext, Entity, TestAppContext};
use ui_c_support::*;
use wispy_core::assistant::{CommentAction, Thread};
use wispydiff::diff::assistant::{self, AssistantPanel};

fn panel(ui: &Ui, cx: &impl AppContext) -> Entity<AssistantPanel> {
    ui.diff(cx)
        .read_with(cx, |d, _| d.assistant.clone().unwrap())
}
fn active(ui: &Ui, cx: &impl AppContext) -> Option<Thread> {
    panel(ui, cx).read_with(cx, |p, _| {
        p.active
            .as_ref()
            .and_then(|id| p.threads.iter().find(|t| t.id == *id))
            .cloned()
    })
}
async fn ask(ui: &Ui, cx: &mut TestAppContext, question: &str) -> Thread {
    let before = active(ui, cx).map(|t| t.messages.len()).unwrap_or(0);
    ui.input(cx, question);
    ui.keys(cx, "enter");
    ui.wait(cx, "assistant completed turn", |cx| {
        active(ui, cx).is_some_and(|t| t.messages.len() >= before + 2)
    })
    .await;
    active(ui, cx).unwrap()
}
async fn open(ui: &Ui, cx: &mut TestAppContext) {
    ui.focus_diff(cx);
    ui.keys(cx, "a");
    ui.wait(cx, "assistant open", |cx| {
        ui.diff(cx).read(cx).assistant.is_some()
    })
    .await;
}
fn new_conversation(ui: &Ui, cx: &mut TestAppContext) {
    // Anchored dropdown starts below the header label; first item center, measured in root.
    let top = ui
        .diff(cx)
        .read_with(cx, |d, _| d.list.viewport_bounds().top());
    ui.click(cx, point(px(1110.), top + px(18.)));
    ui.click(cx, point(px(1110.), top + px(80.)));
}
fn latest_call(f: &Fixture) -> serde_json::Value {
    f.log()
        .into_iter()
        .rev()
        .find(|l| l.get("prompt").is_some())
        .unwrap()
}
fn args(call: &serde_json::Value) -> Vec<&str> {
    call["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}

// e2e: asks about selected lines ...; follows up in the same session;
// marks the asked-about lines in the gutter
#[gpui_kit::test]
async fn selection_context_followup_and_gutter_markers(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    let offsets = ui.changed_offsets(cx, 0);
    assert!(offsets.len() >= 2);
    ui.reveal_row(cx, 0, offsets[0]);
    let from = ui.code_position(cx, 0, offsets[0], 0.);
    let to = ui.code_position(cx, 0, offsets[1], 8.);
    cx.update_window(ui.window, |_, w, cx| {
        gpui_kit::test::TestWindowExt::drag(w, from, to, cx)
    })
    .unwrap();
    assert!(ui.diff(cx).read_with(cx, |d, _| d
        .text_selection
        .as_ref()
        .is_some_and(|s| !s.is_empty())));
    open(&ui, cx).await;
    let thread = ask(&ui, cx, "Why does this threshold change?").await;
    let selection = thread
        .selection
        .as_ref()
        .expect("selected code is sent as context");
    assert_eq!(selection.path, "src/Module0/Service0.php");
    assert_eq!(selection.pr_index, 1);
    assert!(selection.end_line >= selection.start_line);
    assert!(thread
        .messages
        .last()
        .unwrap()
        .text
        .contains("About “Why does this threshold change?”"));
    assert!(thread
        .messages
        .last()
        .unwrap()
        .text
        .contains("**checkout**"));
    let call = latest_call(&f);
    assert_eq!(call["provider"], "claude");
    assert!(args(&call)
        .join(" ")
        .contains("--tools Read,Grep,Glob,ToolSearch --permission-mode bypassPermissions"));
    assert!(call["prompt"]
        .as_str()
        .unwrap()
        .contains("The question is about these lines of src/Module0/Service0.php (in #2"));
    assert!(call["cwd"]
        .as_str()
        .unwrap()
        .contains("/worktrees/wispy/fixture/"));
    let follow = ask(&ui, cx, "And what breaks if it's lower?").await;
    assert_eq!(follow.id, thread.id);
    assert!(follow
        .messages
        .last()
        .unwrap()
        .text
        .starts_with("(follow-up)"));
    let call = latest_call(&f);
    assert!(args(&call).contains(&"--resume"));
    assert_eq!(call["prompt"], "And what breaks if it's lower?");
    ui.diff(cx).read_with(cx, |d, cx| {
        assert_eq!(
            assistant::marker_at(d, &selection.path, selection.head_start.unwrap(), cx),
            Some(thread.id.clone())
        );
    });
    ui.close(cx).await;
}

// e2e: offers to sign in again ... then asks again
#[gpui_kit::test]
async fn expired_signin_opens_browser_and_retries_with_full_context(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    open(&ui, cx).await;
    let thread = ask(&ui, cx, "expired").await;
    let error = thread.messages.last().unwrap();
    assert!(error.error && error.sign_in);
    assert!(error.text.contains("OAuth session expired"));
    let top = ui
        .diff(cx)
        .read_with(cx, |d, _| d.list.viewport_bounds().top());
    ui.click(cx, point(px(1180.), top + px(168.)));
    ui.wait(cx, "fake auth login", |_| {
        f.log()
            .iter()
            .any(|l| l["args"] == serde_json::json!(["auth", "login"]) && l["browser"] == "true")
    })
    .await;
    ui.wait(cx, "browser opened", |_| {
        f.log()
            .iter()
            .any(|l| l["opened"] == "https://claude.example.test/oauth/authorize?state=fake")
    })
    .await;
    let start = std::time::Instant::now();
    loop {
        ui.click(cx, point(px(1160.), top + px(168.)));
        if f.log().iter().filter(|l| l.get("prompt").is_some()).count() >= 2 {
            break;
        }
        assert!(start.elapsed() < TIMEOUT, "Ask again did not retry");
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    ui.wait(cx, "retry completed", |cx| {
        active(&ui, cx).is_some_and(|t| t.messages.iter().filter(|m| m.error).count() == 2)
    })
    .await;
    let call = latest_call(&f);
    assert!(call["prompt"]
        .as_str()
        .unwrap()
        .contains("Question: expired"));
    assert!(call["prompt"]
        .as_str()
        .unwrap()
        .contains("In view: wispy/fixture #2"));
    assert!(!args(&call).contains(&"--resume"));
    ui.close(cx).await;
}

// e2e: asks Codex about the whole range, and shows failures in the thread
#[gpui_kit::test]
async fn codex_whole_range_hidden_files_and_failure_thread(cx: &mut TestAppContext) {
    let f = Fixture::new();
    f.backend.prefs().set_str("assistant.provider", "codex");
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    let files = ui.paths(cx);
    let ext = files
        .iter()
        .map(|f| wispydiff::diff::extension_of(f))
        .find(|e| e != ".php")
        .unwrap();
    // Use the real filter keyboard: move through its sorted extension rows, toggle selected one.
    ui.keys(cx, "f");
    let exts = ui.diff(cx).read_with(cx, |d, _| {
        let mut e: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .map(|f| wispydiff::diff::extension_of(&f.path))
            .collect();
        e.sort();
        e.dedup();
        e
    });
    for _ in 0..exts.iter().position(|e| e == &ext).unwrap() {
        ui.keys(cx, "down");
    }
    ui.keys(cx, "space escape");
    let hidden: Vec<_> = files
        .iter()
        .filter(|p| p.ends_with(&ext))
        .cloned()
        .collect();
    assert_eq!(
        ui.diff(cx)
            .read_with(cx, |d, _| d.filtered().iter().filter(|v| **v).count()),
        hidden.len()
    );
    open(&ui, cx).await;
    let thread = ask(&ui, cx, "Summarize this PR").await;
    assert!(thread.selection.is_none());
    assert!(thread
        .messages
        .last()
        .unwrap()
        .text
        .contains("About “Summarize this PR”"));
    let call = latest_call(&f);
    assert_eq!(call["provider"], "codex");
    assert!(args(&call).contains(&"sandbox_mode=\"read-only\""));
    let prompt = call["prompt"].as_str().unwrap();
    assert!(prompt.contains("In view: wispy/fixture #2"));
    assert!(!prompt.contains("diff --git"));
    for path in hidden {
        assert!(prompt.contains(&format!("\n- {path}\n")));
        assert!(!prompt.contains(&format!("\n- {path} (")));
    }
    assert!(prompt.contains("\n- src/Module0/Service0.php ("));
    let failed = ask(&ui, cx, "fail").await;
    assert_eq!(panel(&ui, cx).read_with(cx, |p, _| p.threads.len()), 1);
    assert_eq!(failed.id, thread.id);
    assert!(failed.messages.last().unwrap().error);
    assert!(failed
        .messages
        .last()
        .unwrap()
        .text
        .contains("model not available (fake)"));
    ui.keys(cx, "escape");
    assert!(ui.diff(cx).read_with(cx, |d, _| d.assistant.is_none()));
    ui.close(cx).await;
}

// e2e: adds the review comments ... drafts ... then edits and deletes them
#[gpui_kit::test]
async fn assistant_mcp_comments_stream_then_edit_and_delete_drafts(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    open(&ui, cx).await;
    ui.input(cx, "comments:wait");
    ui.keys(cx, "enter");
    ui.wait(cx, "first draft while CLI is blocked", |cx| {
        ui.session.read(cx).drafts.len() == 1
    })
    .await;
    assert!(active(&ui, cx).is_none());
    let call = latest_call(&f);
    assert!(call["prompt"]
        .as_str()
        .unwrap()
        .contains("add each one with `add_draft_comment`"));
    f.go();
    ui.wait(cx, "comment turn complete", |cx| {
        active(&ui, cx).is_some_and(|t| t.messages.last().unwrap().comments.len() == 3)
    })
    .await;
    ui.wait(cx, "two drafts", |cx| ui.session.read(cx).drafts.len() == 2)
        .await;
    let thread = active(&ui, cx).unwrap();
    let comments = &thread.messages.last().unwrap().comments;
    assert_eq!(comments.iter().filter(|c| c.error.is_none()).count(), 2);
    assert_eq!(
        comments[2].error.as_deref(),
        Some("nowhere/Missing.php isn't changed in this range")
    );
    assert!(comments[0].body.as_ref().unwrap().contains("**simpler**"));
    assert!(comments[0]
        .body
        .as_ref()
        .unwrap()
        .contains("```suggestion\nsimpler();\n```"));
    assert!(ui
        .session
        .read_with(cx, |s, _| s.drafts.iter().all(|d| d.draft.assistant)));
    let revised = ask(&ui, cx, "revise").await;
    let outcomes = &revised.messages.last().unwrap().comments;
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].action, CommentAction::Edit);
    assert_eq!(outcomes[1].action, CommentAction::Delete);
    assert_eq!(outcomes[2].error.as_deref(), Some("there's no draft d99"));
    let tools: Vec<_> = f
        .log()
        .into_iter()
        .filter_map(|v| v["tool"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        &tools[tools.len() - 4..],
        ["list_drafts", "edit_draft", "delete_draft", "edit_draft"]
    );
    ui.wait(cx, "one revised draft", |cx| {
        ui.session.read(cx).drafts.len() == 1
            && ui.session.read(cx).drafts[0]
                .draft
                .body
                .starts_with("Revised: This could be")
    })
    .await;
    // The draft card is a real virtual-list insert. Reveal it, then click its Delete button.
    ui.focus_diff(cx);
    ui.keys(cx, "escape");
    let diff = ui.diff(cx);
    let item = diff.read_with(cx, |d, _| {
        d.built
            .items
            .iter()
            .position(|item| {
                matches!(item, wispydiff::diff::layout::Item::Insert(n)
                if d.built.inserts[*n as usize].key.starts_with("draft:"))
            })
            .unwrap()
    });
    diff.update(cx, |d, cx| {
        d.list.scroll_to(gpui_kit::ListOffset {
            item_ix: item,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    ui.frame(cx);
    let bounds = diff.read_with(cx, |d, _| d.list.bounds_for_item(item).unwrap());
    ui.click(
        cx,
        point(bounds.left() + px(205.), bounds.bottom() - px(30.)),
    );
    ui.wait(cx, "draft deleted by reader", |cx| {
        ui.session.read(cx).drafts.is_empty()
    })
    .await;
    ui.close(cx).await;
}

// e2e: shows a new conversation from its start after scrolling down a long one
#[gpui_kit::test]
async fn new_conversation_starts_at_top_after_long_answer(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    open(&ui, cx).await;
    ask(
        &ui,
        cx,
        &format!("say:{}", "A long answer.\\n\\n".repeat(150)),
    )
    .await;
    new_conversation(&ui, cx);
    ui.wait(cx, "new conversation selected", |cx| {
        panel(&ui, cx).read(cx).active.is_none()
    })
    .await;
    let short = ask(&ui, cx, "Short question").await;
    assert_eq!(short.messages.len(), 2);
    assert_eq!(short.messages[0].text, "Short question");
    let shown = ui.snapshots(cx);
    let key = format!("msg-{}-1", short.id);
    let answer = shown
        .iter()
        .filter(|s| format!("{:?}", s.path()).contains(&key))
        .find(|s| s.visible())
        .expect("short answer painted in conversation viewport");
    let input = shown
        .iter()
        // The conversation has started: the question box asks for a follow-up.
        .find(|s| s.label() == Some("Follow up… (↵ to send, ⇧↵ for a new line)"))
        .unwrap();
    let top = ui
        .diff(cx)
        .read_with(cx, |d, _| d.list.viewport_bounds().top());
    // One 21px question line + its 20px padding, 8px message gap, 10px answer padding.
    assert!(
        answer.bounds().top() - px(59.) >= top + px(43.),
        "the first question is fully below the conversation header"
    );
    assert!(answer.bounds().bottom() < input.bounds().top());
    assert_eq!(panel(&ui, cx).read_with(cx, |p, _| p.threads.len()), 2);
    ui.click(cx, point(px(1370.), top + px(18.)));
    ui.wait(cx, "active conversation deleted", |cx| {
        panel(&ui, cx).read(cx).active.is_none() && panel(&ui, cx).read(cx).threads.len() == 1
    })
    .await;

    ui.close(cx).await;
}

// e2e: keeps the panel and a question being answered when the diff reloads
#[gpui_kit::test]
async fn reload_keeps_streaming_question_and_unsent_input(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    open(&ui, cx).await;
    ui.input(cx, "tools:[[\"wait\"]]");
    ui.keys(cx, "enter");
    ui.wait(cx, "CLI blocked", |_| {
        f.log().iter().any(|l| {
            l["prompt"]
                .as_str()
                .is_some_and(|p| p.contains("tools:[[\"wait\"]]"))
        })
    })
    .await;
    ui.input(cx, "unsent");
    let old = panel(&ui, cx);
    ui.keys(cx, &secondary("i"));
    ui.open(cx, 2).await;
    ui.wait(cx, "panel reopened", |cx| {
        ui.diff(cx).read(cx).assistant.is_some()
    })
    .await;
    assert_eq!(panel(&ui, cx), old);
    f.go();
    ui.wait(cx, "answer arrives in retained panel", |cx| {
        active(&ui, cx).is_some()
    })
    .await;
    assert_eq!(
        active(&ui, cx).unwrap().messages[0].text,
        "tools:[[\"wait\"]]"
    );
    // The retained textarea must still submit exactly its unsent text after returning.
    ui.click(cx, point(px(1200.), px(792.)));
    ui.keys(cx, "enter");
    ui.wait(cx, "unsent question retained", |cx| {
        active(&ui, cx).is_some_and(|t| {
            t.messages
                .iter()
                .any(|m| m.role == "user" && m.text == "unsent")
        })
    })
    .await;
    ui.close(cx).await;
}

// e2e: links file references in answers ... diff, or file view outside it.
// The rich-text links expose their navigation through AssistantPanelEvent::OpenRef. We send
// that public event after a real fake-CLI answer; link target parsing/resolution is checked
// separately below because individual inline links do not expose headless bounds.
#[gpui_kit::test]
async fn answer_references_ranges_bare_absolute_outside_hidden_and_wrapped_jump(
    cx: &mut TestAppContext,
) {
    use wispydiff::{
        diff::{assistant::AssistantPanelEvent, layout::Mode},
        logic::file_refs::{parse_line_list, parse_ref, resolve_line_list, CodeRef, RefPart},
    };
    let f = Fixture::new();
    f.backend.prefs().set_str("assistant.provider", "codex");
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    let files = ui.paths(cx);
    let hidden_ext = files
        .iter()
        .map(|p| wispydiff::diff::extension_of(p))
        .find(|e| e != ".php")
        .unwrap();
    ui.keys(cx, "f");
    let exts = ui.diff(cx).read_with(cx, |d, _| {
        let mut v: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .map(|f| wispydiff::diff::extension_of(&f.path))
            .collect();
        v.sort();
        v.dedup();
        v
    });
    for _ in 0..exts.iter().position(|e| e == &hidden_ext).unwrap() {
        ui.keys(cx, "down");
    }
    ui.keys(cx, "space escape");
    open(&ui, cx).await;
    let thread = ask(&ui, cx, "refs").await;
    let text = &thread.messages.last().unwrap().text;
    let codes: Vec<_> = text
        .split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s)
        .collect();
    let mut references = Vec::new();
    let mut plain = Vec::new();
    let mut unresolved = Vec::new();
    // Use the same parser/resolver as rendered code references and actual head-file existence.
    let known = |path: &str| {
        files
            .iter()
            .any(|p| p == path || p.ends_with(&format!("/{path}")))
    };
    for code in &codes {
        if let Some(parts) = parse_line_list(code) {
            for p in parts {
                if let RefPart::Reference { reference, .. } = p {
                    unresolved.push(reference.path);
                }
            }
        } else if let Some(r) = parse_ref(code) {
            if !known(&r.path) {
                unresolved.push(r.path);
            }
        }
    }
    let (stack, hi) = ui.diff(cx).read_with(cx, |d, _| (d.stack_id.clone(), d.hi));
    let exists = f
        .backend
        .existing_files(stack, hi, unresolved)
        .await
        .unwrap();
    for code in codes {
        if let Some(parts) = parse_line_list(code).and_then(|parts| {
            resolve_line_list(&parts, |r| {
                wispydiff::logic::file_refs::resolve_ref(r, &files, |p| {
                    exists.iter().any(|e| e == p)
                })
            })
        }) {
            for part in parts {
                if let RefPart::Reference { reference, .. } = part {
                    references.push(reference);
                }
            }
        } else if let Some(r) = parse_ref(code).and_then(|r| {
            wispydiff::logic::file_refs::resolve_ref(&r, &files, |p| exists.iter().any(|e| e == p))
        }) {
            references.push(r);
        } else {
            plain.push(code.to_string());
        }
    }
    assert_eq!(references.len(), 8);
    let (range, bare, absolute, outside, hidden) = (
        &references[0],
        &references[1],
        &references[2],
        &references[3],
        &references[4],
    );
    assert!(range.path.starts_with("src/"));
    assert_eq!(range.end, range.line.map(|l| l + 1));
    assert_eq!(
        bare,
        &CodeRef {
            path: range.path.clone(),
            line: None,
            end: None
        }
    );
    assert_eq!(
        absolute,
        &CodeRef {
            path: range.path.clone(),
            line: range.line,
            end: None
        }
    );
    assert!(!files.contains(&outside.path));
    assert!(hidden.path.ends_with(&hidden_ext));
    assert_eq!(references[5].line, range.line);
    assert_eq!(references[6].line, range.line.map(|l| l + 1));
    assert_eq!(references[6].end, range.line.map(|l| l + 2));
    assert_eq!(references[7].path, range.path);
    assert_eq!(
        plain,
        [
            "this.state".to_string(),
            format!("{}/*.php", range.path.rsplit_once('/').unwrap().0),
            "order/germany-gross-order-import/order-create".to_string()
        ]
    );
    let navigate = |ui: &Ui, cx: &mut TestAppContext, r: &CodeRef| {
        panel(ui, cx).update(cx, |_, cx| cx.emit(AssistantPanelEvent::OpenRef(r.clone())))
    };
    navigate(&ui, cx, range);
    ui.wait(cx, "range reference flashes two lines", |cx| {
        ui.diff(cx).read(cx).flash.is_some_and(|r| r.to > r.from)
    })
    .await;
    assert!(ui.diff(cx).read_with(cx, |d, _| d.assistant.is_some()));
    ui.focus_diff(cx);
    ui.keys(cx, "z");
    ui.diff(cx).update(cx, |d, cx| {
        d.list.scroll_to(gpui_kit::ListOffset {
            item_ix: d.built.items.len() - 1,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    let unchanged = &references[7];
    navigate(&ui, cx, unchanged);
    ui.wait(cx, "wrapped unchanged reference visible in split", |cx| {
        let d = ui.diff(cx);
        let d = d.read(cx);
        d.flash.is_some_and(|r| {
            r.mode == Mode::Split
                && d.list
                    .bounds_for_item(d.built.item_of_row(d.segment(r.file).start + r.from))
                    .is_some_and(|b| {
                        b.top() >= d.list.viewport_bounds().top()
                            && b.bottom() <= d.list.viewport_bounds().bottom()
                    })
        })
    })
    .await;
    for (reference, filtered) in [(outside, false), (hidden, true)] {
        navigate(&ui, cx, reference);
        ui.wait(cx, "read-only reference loaded", |cx| {
            ui.diff(cx)
                .read(cx)
                .file_view
                .as_ref()
                .is_some_and(|v| v.path == reference.path && v.lines.is_some())
        })
        .await;
        ui.diff(cx).read_with(cx, |d, _| {
            let v = d.file_view.as_ref().unwrap();
            assert_eq!(v.filtered, filtered);
            assert_eq!(
                (v.line, v.end),
                (reference.line.unwrap(), reference.line.unwrap())
            );
        });
        ui.keys(cx, "escape");
        assert!(ui.diff(cx).read_with(cx, |d, _| d.file_view.is_none()));
    }
    ui.close(cx).await;
}
