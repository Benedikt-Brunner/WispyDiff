//! Review comments the assistant writes become drafts. Its own test binary: it points the CLI
//! variables at the fakes without racing the other assistant tests' logs.

mod support;

use std::path::PathBuf;

use support::{pull_request, OriginRepo};
use wispy_core::assistant::{review_blocks, ProposedComment, Provider, ReviewBlock};
use wispy_core::cache::Cache;
use wispy_core::assistant::CommentAction;
use wispy_core::drafts::DraftKind;
use wispy_core::github::{GitHubClient, Side};
use wispy_core::highlight::Highlighter;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::{NewDraft, NewThread, PrService};
use wispy_core::stack::Stack;

#[test]
fn finds_review_comment_blocks_but_not_inside_other_code_blocks() {
    let answer = "Intro\n\
        ````review-comment src/a.ts:3-4\n\
        Use this:\n\
        ```suggestion\n\
        x();\n\
        ```\n\
        ````\n\
        ```text\n\
        ```review-comment src/quoted.ts:1\n\
        not a comment\n\
        ```\n\
        ~~~review-comment src/b.ts#L7-L9\n\
        Tilde.\n\
        ~~~\n\
        ```review-comment src/c.ts\n\
        Whole file.\n\
        ```\n\
        ```review-comment\n\
        No target.\n\
        ```\n\
        ````review-comment-edit d2\n\
        New text.\n\
        ````\n\
        ```review-comment-delete d3\n\
        ```\n\
        ```review-comment-delete\n\
        ```\n\
        ```review-comment src/d.ts:5\n\
        Never closed.";
    let comment = |path: &str, lines, body: &str| ReviewBlock::Comment(ProposedComment { path: path.into(), lines, body: body.into() });
    assert_eq!(
        review_blocks(answer),
        vec![
            comment("src/a.ts", Some((3, 4)), "Use this:\n```suggestion\nx();\n```"),
            comment("src/b.ts", Some((7, 9)), "Tilde."),
            comment("src/c.ts", None, "Whole file."),
            ReviewBlock::Edit { draft: "d2".into(), body: "New text.".into() },
            ReviewBlock::Delete { draft: "d3".into() },
            comment("src/d.ts", Some((5, 5)), "Never closed."),
        ]
    );
}

#[test]
fn adds_the_assistants_comments_on_the_prs_their_lines_belong_to_then_edits_them_by_ref() {
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../e2e/fake-cli/claude").canonicalize().unwrap();
    let log = tempfile::NamedTempFile::new().unwrap();
    // SAFETY: the only test in this binary that reads them.
    unsafe {
        std::env::set_var("WISPY_CLAUDE_BIN", fake);
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

    let block = |target: &str, body: &str| format!("```review-comment {target}\\n{body}\\n```\\n");
    let answer = [
        block("src/Order.php:3", "on #1's line"),
        block("src/Order.php:6", "on #2's line"),
        block("src/Order.php:4-6", "unchanged lines and #2's"),
        block("src/Order.php:3-6", "spans both PRs"),
        block("src/Order.php", "the whole file"),
        block("src/Order.php:99", "past the end"),
        block("README.md:1", "not in the diff"),
    ]
    .concat();
    let new = NewThread { provider: Provider::Claude, model: None, effort: None, selection: None, anchor: None, hidden: vec![] };
    let thread = service.ask(&snapshot, (0, 1), None, Some(new), &format!("say:{answer}"), |_| {}).unwrap();
    let message = thread.messages.last().unwrap();
    assert!(!message.error, "{}", message.text);
    let errors: Vec<_> = message.comments.iter().map(|c| c.error.as_deref()).collect();
    assert_eq!(
        errors,
        [None, None, None, None, None, Some("src/Order.php has no line 99 at the head"), Some("README.md isn't changed in this range")]
    );

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

    // The next turn lists the review's drafts by ref (the reviewer's own too); an answer without
    // comment blocks changes nothing.
    let mine = service
        .create_draft(
            &snapshot,
            NewDraft { pr_index: 1, kind: DraftKind::Summary, path: None, side: None, line: None, start_line: None, body: "Looks good\noverall".into(), thread_id: None, reply_to: None, assistant: false },
        )
        .unwrap();
    let followed = service.ask(&snapshot, (0, 1), Some(&thread.id), None, "And otherwise?", |_| {}).unwrap();
    assert!(followed.messages.last().unwrap().comments.is_empty());
    let prompt = last_prompt(log.path());
    assert!(prompt.starts_with("The review's draft comments (not posted yet), by ref:\n"), "{prompt}");
    assert!(prompt.contains("\n[d1] src/Order.php L2 in #1 — by you:\n> on #1's line\n"), "{prompt}");
    assert!(prompt.contains("\n[d3] src/Order.php L4–6 in #2 — by you:\n"), "{prompt}");
    assert!(prompt.contains("\n[d5] src/Order.php (whole file) in #2 — by you:\n"), "{prompt}");
    assert!(prompt.contains("\n[d6] the review summary of #2 — by the reviewer:\n> Looks good\n> overall\n"), "{prompt}");
    assert!(prompt.ends_with("\nQuestion: And otherwise?\n"));
    // Unchanged drafts aren't sent again.
    service.ask(&snapshot, (0, 1), Some(&thread.id), None, "Anything else?", |_| {}).unwrap();
    assert_eq!(last_prompt(log.path()), "Anything else?");

    // Edits and deletes by ref; a draft the reviewer changes while the assistant answers is left alone.
    let ids: Vec<String> = service.shown_drafts(&snapshot).unwrap().into_iter().map(|d| d.draft.id).collect();
    let answer = [
        "```review-comment-edit d1\\nsharper\\n```\\n",
        "```review-comment-edit d2\\nmine now\\n```\\n",
        "```review-comment-delete d3\\n```\\n",
        "```review-comment-edit d9\\nwho?\\n```\\n",
    ]
    .concat();
    let turn = std::thread::scope(|s| {
        let turn = s.spawn(|| service.ask(&snapshot, (0, 1), Some(&thread.id), None, &format!("wait:say:{answer}"), |_| {}).unwrap());
        while !last_prompt(log.path()).starts_with("wait:") {
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
            (CommentAction::Edit, "d2", Some("src/Order.php L6 in #2"), Some("the reviewer changed d2 meanwhile")),
            (CommentAction::Delete, "d3", Some("src/Order.php L4–6 in #2"), None),
            (CommentAction::Edit, "d9", None, Some("there's no draft d9")),
        ]
    );
    let bodies: Vec<_> = service.shown_drafts(&snapshot).unwrap().into_iter().map(|d| (d.draft.id, d.draft.body)).collect();
    assert_eq!(bodies[0], (ids[0].clone(), "sharper".to_string()));
    assert_eq!(bodies[1], (ids[1].clone(), "reviewer's words".to_string()));
    assert_eq!(bodies.len(), 5, "d3 is gone");
    assert!(!bodies.iter().any(|(id, _)| *id == ids[2]));
    assert!(bodies.iter().any(|(id, _)| *id == mine.id));

    // The next listing has the reviewer's change and keeps the refs (d3 is gone); deleting d3 again fails.
    service.ask(&snapshot, (0, 1), Some(&thread.id), None, "say:```review-comment-delete d3\\n```", |_| {}).unwrap();
    let prompt = last_prompt(log.path());
    assert!(prompt.contains("\n[d2] src/Order.php L6 in #2 — by you:\n> reviewer's words\n") && prompt.contains("\n[d1] src/Order.php L2 in #1 — by you:\n> sharper\n"), "{prompt}");
    assert!(!prompt.contains("[d3]") && prompt.contains("[d6]"), "{prompt}");
    let thread = service.assistant_threads(&snapshot).unwrap().into_iter().find(|t| t.id == thread.id).unwrap();
    assert_eq!(thread.messages.last().unwrap().comments[0].error.as_deref(), Some("d3 is no longer a draft"));
}

fn last_prompt(log: &std::path::Path) -> String {
    let entries = std::fs::read_to_string(log).unwrap_or_default();
    let last = entries.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|c| c.get("prompt").is_some()).last();
    last.map(|c| c["prompt"].as_str().unwrap().to_string()).unwrap_or_default()
}
