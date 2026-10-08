//! The assistant's review tools: comments it adds become drafts, and it edits and deletes drafts
//! by ref. Its own test binary: it points the CLI variables at the fakes without racing the other
//! assistant tests' logs.

mod support;

use std::path::PathBuf;

use serde_json::json;
use support::{pull_request, OriginRepo};
use wispy_core::assistant::{AssistantEvent, CommentAction, Provider};
use wispy_core::cache::Cache;
use wispy_core::drafts::DraftKind;
use wispy_core::github::{GitHubClient, Side};
use wispy_core::highlight::Highlighter;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::{NewDraft, NewThread, PrService};
use wispy_core::stack::Stack;

#[test]
fn adds_the_assistants_comments_on_the_prs_their_lines_belong_to_then_edits_them_by_ref() {
    let log = tempfile::NamedTempFile::new().unwrap();
    // SAFETY: the only test in this binary that reads them.
    unsafe {
        std::env::set_var("WISPY_CLAUDE_BIN", fake("claude"));
        std::env::set_var("WISPY_CODEX_BIN", fake("codex"));
        std::env::set_var("WISPY_FAKE_LOG", log.path());
    }

    let origin = OriginRepo::init();
    origin.write("src/Order.php", "<?php\na\nb\nc\nd\ne\n");
    origin.write("README.md", "# Shop\n");
    origin.commit("base");
    origin.checkout_new("s/1");
    origin.write("src/Order.php", "<?php\nA1\nb\nc\nd\ne\n");
    origin.commit("one");
    origin.publish_pr(1, "s/1");
    origin.checkout_new("s/2");
    // A line inserted at the top: #1's lines are one further down at the head.
    origin.write("src/Order.php", "<?php\n// top\nA1\nb\nc\nD2\ne\n");
    origin.commit("two");
    origin.publish_pr(2, "s/2");
    let url = origin.url();
    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let snapshot = service.fetch(Stack { prs: vec![pull_request(1, "main", "s/1", &url), pull_request(2, "s/1", "s/2", &url)], focus: 0 }).unwrap();

    let add = |path: &str, lines: Option<(u32, u32)>, body: &str| {
        let mut args = json!({ "path": path, "body": body });
        if let Some((start, end)) = lines {
            args["start_line"] = json!(start);
            args["end_line"] = json!(end);
        }
        json!(["add_draft_comment", args])
    };
    let steps = json!([
        add("src/Order.php", Some((3, 3)), "on #1's line"),
        add("src/Order.php", Some((6, 6)), "on #2's line"),
        add("src/Order.php", Some((4, 6)), "unchanged lines and #2's"),
        add("src/Order.php", Some((3, 6)), "spans both PRs"),
        add("src/Order.php", None, "the whole file"),
        add("src/Order.php", Some((99, 99)), "past the end"),
        add("README.md", Some((1, 1)), "not in the diff"),
        add("src/Order.php", Some((5, 4)), "backwards"),
    ]);
    let new = NewThread { provider: Provider::Claude, model: None, effort: None, selection: None, anchor: None, hidden: vec![] };
    let mut streamed = Vec::new();
    let thread = service
        .ask(&snapshot, (0, 1), None, Some(new), &format!("tools:{steps}"), |event| {
            if let AssistantEvent::Comment { outcome } = event {
                streamed.push(outcome.clone());
            }
        })
        .unwrap();
    let message = thread.messages.last().unwrap();
    assert!(!message.error, "{}", message.text);
    // The agent hears where each comment went, or why it couldn't go there.
    assert_eq!(
        message.text.lines().collect::<Vec<_>>(),
        [
            "add_draft_comment: Added draft d1 on src/Order.php L2 in #1.",
            "add_draft_comment: Added draft d2 on src/Order.php L6 in #2.",
            "add_draft_comment: Added draft d3 on src/Order.php L4–6 in #2.",
            "add_draft_comment: Added draft d4 on src/Order.php L6 in #2.",
            "add_draft_comment: Added draft d5 on src/Order.php (whole file) in #2.",
            "add_draft_comment: error: src/Order.php has no line 99 at the head",
            "add_draft_comment: error: README.md isn't changed in this range",
            "add_draft_comment: error: 5–4 isn't a line range",
        ]
    );
    assert_eq!(streamed, message.comments, "each change is streamed as it happens");
    let errors: Vec<_> = message.comments.iter().map(|c| c.error.is_some()).collect();
    assert_eq!(errors, [false, false, false, false, false, true, true, true]);

    let drafts = service.shown_drafts(&snapshot).unwrap();
    let placed: Vec<_> = drafts.iter().map(|d| (d.draft.pr, d.draft.kind, d.draft.line, d.draft.start_line, d.draft.body.as_str())).collect();
    assert_eq!(
        placed,
        [
            (1, DraftKind::Line, Some(2), None, "on #1's line"),
            (2, DraftKind::Line, Some(6), None, "on #2's line"),
            (2, DraftKind::Line, Some(6), Some(4), "unchanged lines and #2's"),
            (2, DraftKind::Line, Some(6), None, "spans both PRs"),
            (2, DraftKind::File, None, None, "the whole file"),
        ]
    );
    assert!(drafts.iter().all(|d| d.draft.assistant && d.draft.path.as_deref() == Some("src/Order.php")));
    assert!(drafts.iter().filter(|d| d.draft.kind == DraftKind::Line).all(|d| d.draft.side == Some(Side::Right)));
    assert_eq!(drafts[0].draft.commit, snapshot.heads[0]);
    let ids: Vec<_> = message.comments.iter().filter_map(|c| c.draft.clone()).collect();
    assert_eq!(ids, drafts.iter().map(|d| d.draft.id.clone()).collect::<Vec<_>>());

    // A follow-up (on Codex too: refs belong to the thread) edits a draft it wrote without listing
    // first, and lists the review's drafts by ref, the reviewer's own too.
    let mine = service
        .create_draft(
            &snapshot,
            NewDraft { pr_index: 1, kind: DraftKind::Summary, path: None, side: None, line: None, start_line: None, body: "Looks good\noverall".into(), thread_id: None, reply_to: None, assistant: false },
        )
        .unwrap();
    let steps = json!([["edit_draft", { "ref": "d5", "body": "the whole file, really" }], ["list_drafts", {}]]);
    let followed = service.ask(&snapshot, (0, 1), Some(&thread.id), None, &format!("tools:{steps}"), |_| {}).unwrap();
    let answer = &followed.messages.last().unwrap().text;
    assert!(answer.starts_with("edit_draft: Updated d5.\nlist_drafts: The review's draft comments (not posted yet), by ref:\n"), "{answer}");
    assert!(answer.contains("\n[d1] src/Order.php L2 in #1 — by you:\n> on #1's line\n"), "{answer}");
    assert!(answer.contains("\n[d5] src/Order.php (whole file) in #2 — by you:\n> the whole file, really\n"), "{answer}");
    assert!(answer.contains("\n[d6] the review summary of #2 — by the reviewer:\n> Looks good\n> overall"), "{answer}");
    assert_eq!(last_prompt(log.path()), format!("tools:{steps}"), "follow-ups send just the question");

    // Edits and deletes by ref; a draft the reviewer changes after the assistant saw it is left alone.
    let steps = json!([
        ["wait"],
        ["edit_draft", { "ref": "d1", "body": "sharper" }],
        ["edit_draft", { "ref": "d2", "body": "mine now" }],
        ["delete_draft", { "ref": "d3" }],
        ["edit_draft", { "ref": "d6", "body": "Looks good overall" }],
        ["edit_draft", { "ref": "d9", "body": "who?" }],
    ]);
    let turn = std::thread::scope(|s| {
        let turn = s.spawn(|| service.ask(&snapshot, (0, 1), Some(&thread.id), None, &format!("tools:{steps}"), |_| {}).unwrap());
        while last_prompt(log.path()) != format!("tools:{steps}") {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        service.update_draft(&ids[1], Some("reviewer's words".into()), None).unwrap();
        std::fs::write(format!("{}.go", log.path().display()), "").unwrap();
        turn.join().unwrap()
    });
    let outcomes: Vec<_> = turn.messages.last().unwrap().comments.iter().map(|c| (c.action, c.target.as_deref().unwrap(), c.label.as_deref(), c.error.as_deref())).collect();
    assert_eq!(
        outcomes,
        [
            (CommentAction::Edit, "d1", Some("src/Order.php L2 in #1"), None),
            (CommentAction::Edit, "d2", Some("src/Order.php L6 in #2"), Some("the reviewer changed d2 since you last saw it")),
            (CommentAction::Delete, "d3", Some("src/Order.php L4–6 in #2"), None),
            (CommentAction::Edit, "d6", Some("the review summary of #2"), None),
            (CommentAction::Edit, "d9", None, Some("there's no draft d9")),
        ]
    );
    let bodies: Vec<_> = service.shown_drafts(&snapshot).unwrap().into_iter().map(|d| (d.draft.id, d.draft.body)).collect();
    assert_eq!(bodies[0], (ids[0].clone(), "sharper".to_string()));
    assert_eq!(bodies[1], (ids[1].clone(), "reviewer's words".to_string()));
    assert_eq!(bodies.len(), 5, "d3 is gone");
    assert!(!bodies.iter().any(|(id, _)| *id == ids[2]));
    assert!(bodies.contains(&(mine.id.clone(), "Looks good overall".to_string())));

    // Once listed again, the reviewer's change can be edited; refs stay (d3 is gone for good).
    let steps = json!([["delete_draft", { "ref": "d3" }], ["edit_draft", { "ref": "d2", "body": "both of us" }], ["list_drafts", {}], ["edit_draft", { "ref": "d2", "body": "both of us" }]]);
    service.ask(&snapshot, (0, 1), Some(&thread.id), None, &format!("tools:{steps}"), |_| {}).unwrap();
    let thread = service.assistant_threads(&snapshot).unwrap().into_iter().find(|t| t.id == thread.id).unwrap();
    let answer = &thread.messages.last().unwrap().text;
    assert!(answer.starts_with("delete_draft: error: d3 is no longer a draft\n"), "{answer}");
    assert!(answer.contains("\nedit_draft: error: the reviewer changed d2 since you last saw it (list_drafts shows its text now)\n"), "the agent hears how to go on: {answer}");
    assert!(answer.contains("\n[d2] src/Order.php L6 in #2 — by you:\n> reviewer's words\n") && !answer.contains("[d3]"), "{answer}");
    assert!(answer.ends_with("\nedit_draft: Updated d2."), "{answer}");

    // Codex gets the same tools (through its `-c mcp_servers.wispy=…` and the token variable).
    let new = NewThread { provider: Provider::Codex, model: None, effort: None, selection: None, anchor: None, hidden: vec![] };
    let steps = json!([["add_draft_comment", { "path": "src/Order.php", "start_line": 6, "body": "Why D2?" }]]);
    let codex = service.ask(&snapshot, (0, 1), None, Some(new), &format!("tools:{steps}"), |_| {}).unwrap();
    assert_eq!(codex.messages.last().unwrap().text, "add_draft_comment: Added draft d1 on src/Order.php L6 in #2.");
    assert!(service.shown_drafts(&snapshot).unwrap().iter().any(|d| d.draft.body == "Why D2?"));
}

fn fake(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../e2e/fake-cli").join(name).canonicalize().unwrap()
}

fn last_prompt(log: &std::path::Path) -> String {
    let entries = std::fs::read_to_string(log).unwrap_or_default();
    let last = entries.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|c| c.get("prompt").is_some()).last();
    last.map(|c| c["prompt"].as_str().unwrap().to_string()).unwrap_or_default()
}
