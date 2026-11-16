use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::executor::block_on;
use wispydiff::backend::{self, Backend, Range};

// Restore process-global overrides and reap the fixture server even if an assertion fails.
struct FixtureProcess {
    root: PathBuf,
    server: Option<Child>,
    env: Vec<(&'static str, Option<OsString>)>,
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            let _ = server.wait();
        }
        for (key, value) in &self.env {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn backend_commands_and_jobs_from_a_non_tokio_executor() {
    let workspace = if std::path::Path::new(file!()).is_absolute() {
        // Also supports an external test harness while the UI is being edited.
        PathBuf::from(file!())
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut fixture = FixtureProcess {
        root: std::env::temp_dir()
            .join(format!("wispydiff-backend-{}-{nonce}", std::process::id())),
        server: None,
        env: Vec::new(),
    };
    let generated = fixture.root.join("fixture");
    let data = fixture.root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let output = Command::new(env!("CARGO"))
        .current_dir(&workspace)
        .args(["run", "-p", "wispy-fixtures", "--", "generate", "--out"])
        .arg(&generated)
        .args(["--seed", "42"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture generation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(generated.join("fixture.json")).unwrap()).unwrap();
    let input = format!(
        "{}/{}#{}",
        manifest["owner"].as_str().unwrap(),
        manifest["repo"].as_str().unwrap(),
        manifest["prs"][0]["number"]
    );
    assert_eq!(input, "wispy/fixture#1");

    let metadata = Command::new(env!("CARGO"))
        .current_dir(&workspace)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let executable =
        PathBuf::from(metadata["target_directory"].as_str().unwrap()).join("debug/wispy-fixtures");
    let mut command = Command::new(executable);
    command.args(["serve", "--fixture"]).arg(&generated);
    let (server, port) = spawn_fixture_server(command);
    fixture.server = Some(server);

    for (key, value) in [
        ("WISPY_DATA_DIR", data.into_os_string()),
        (
            "WISPY_GITHUB_API",
            format!("http://127.0.0.1:{port}").into(),
        ),
        ("WISPY_GITHUB_TOKEN", "fixture-token".into()),
        (
            "WISPY_CLAUDE_BIN",
            workspace.join("testing/fake-cli/claude").into_os_string(),
        ),
        (
            "WISPY_OPENER",
            workspace.join("testing/fake-cli/open").into_os_string(),
        ),
        (
            "WISPY_FAKE_LOG",
            fixture.root.join("assistant.jsonl").into_os_string(),
        ),
        ("WISPY_PREFETCH_INTERVAL", "3600".into()),
    ] {
        fixture.env.push((key, std::env::var_os(key)));
        std::env::set_var(key, value);
    }
    // Construct the service under test directly, after setting all environment overrides.
    let backend = Backend::new(backend::default_data_dir());
    let (sender, receiver) = mpsc::channel();
    backend.set_on_range_ready(move |ready| {
        let _ = sender.send((ready, std::thread::current().id()));
    });
    backend.warm_up();
    let opened = block_on(backend.open_pr(input.clone())).unwrap();
    assert_eq!(opened.prs.len(), 4);
    assert_eq!(opened.needs_rebase.len(), 4);
    assert_eq!(opened.snapshot.len(), 4);
    assert_eq!(opened.focus, 0);
    assert_eq!(opened.range, Range { lo: 0, hi: 0 });
    assert!(!opened.from_cache);
    assert!(!opened.view.summary.files.is_empty());
    assert!(!opened.view.rows.is_empty());
    assert!(Arc::ptr_eq(
        &backend.view(&opened.view_id).unwrap(),
        &opened.view
    ));

    let range = block_on(backend.select_range(opened.stack_id.clone(), 0, 3, false)).unwrap();
    assert_eq!(range.range, Range { lo: 0, hi: 3 });
    assert!(!range.ignore_whitespace);
    assert!(!range.view.rows.is_empty());
    let cached = backend.cached_range(&opened.stack_id, 0, 3, false).unwrap();
    assert!(Arc::ptr_eq(&range.view, &cached.view));
    assert!(backend.split_file_cached(&range.view_id, 0).is_none());
    let rows = block_on(backend.split_file(range.view_id.clone(), 0)).unwrap();
    assert!(!rows.is_empty());
    assert!(Arc::ptr_eq(
        &backend.split_file_cached(&range.view_id, 0).unwrap(),
        &rows
    ));
    assert!(Arc::ptr_eq(
        &block_on(backend.split_file(range.view_id.clone(), 0)).unwrap(),
        &rows
    ));

    let index = backend.symbol_index(&range.view_id).unwrap();
    assert!(Arc::ptr_eq(
        &index,
        &backend.symbol_index(&range.view_id).unwrap()
    ));
    let mut ready_ranges = HashSet::new();
    while ready_ranges.len() < 10 {
        let (ready, thread) = receiver.recv_timeout(Duration::from_secs(120)).unwrap();
        assert_eq!(ready.stack_id, opened.stack_id);
        assert_ne!(thread, std::thread::current().id());
        ready_ranges.insert((ready.lo, ready.hi));
    }
    assert!(ready_ranges.contains(&(0, 3)));
    assert!(
        block_on(backend.refresh_pr(input.clone(), opened.stack_id.clone()))
            .unwrap()
            .is_none()
    );
    let cached = block_on(backend.open_pr(input)).unwrap();
    assert!(cached.from_cache);
    assert_eq!(cached.stack_id, opened.stack_id);

    // The rest of the command surface uses the same real clone and state, just as the UI does.
    use backend::GrepEvent;
    use wispy_core::anchors::Anchor;
    use wispy_core::assistant::{AssistantEvent, Provider};
    use wispy_core::drafts::DraftKind;
    use wispy_core::github::{Side, Verdict};
    use wispy_core::merge::{MergeMethod, MergeOutcome, MergeRequest};
    use wispy_core::service::{NewDraft, NewThread};

    let stack_id = opened.stack_id.clone();
    let repo = opened.snapshot.repo_slug().to_string();
    assert!(!opened.ignore_whitespace);
    assert!(range.since.is_none());
    let keys = block_on(backend.range_keys(stack_id.clone(), 0, 3, false)).unwrap();
    assert_eq!(keys.len(), range.view.summary.files.len());
    let file = &range.view.summary.files[0];
    assert_eq!(keys[&file.path], file.content_key);
    assert!(block_on(backend.get_viewed(repo.clone()))
        .unwrap()
        .is_empty());
    block_on(backend.set_viewed(repo.clone(), file.content_key.clone(), true)).unwrap();
    assert_eq!(
        block_on(backend.get_viewed(repo.clone())).unwrap(),
        [file.content_key.clone()]
    );
    block_on(backend.set_viewed(repo.clone(), file.content_key.clone(), false)).unwrap();
    assert!(block_on(backend.get_viewed(repo.clone()))
        .unwrap()
        .is_empty());

    let checkpoint = block_on(backend.mark_reviewed(stack_id.clone(), 0, 3)).unwrap();
    assert_eq!(checkpoint.source, "manual");
    assert_eq!(checkpoint.entries.len(), 4);
    assert!(block_on(backend.list_checkpoints(stack_id.clone(), 0, 99))
        .unwrap()
        .contains(&checkpoint));
    let since =
        block_on(backend.select_since(stack_id.clone(), 0, 3, checkpoint.id.clone())).unwrap();
    let info = since.since.unwrap();
    assert_eq!(info.checkpoint_id, checkpoint.id);
    assert_eq!(info.created_at, checkpoint.created_at);
    assert!(!info.conflicts);
    assert_eq!(info.full_keys, keys);
    assert!(since.view.summary.files.is_empty());
    let error = block_on(backend.select_since(stack_id.clone(), 0, 3, "missing".into()))
        .err()
        .unwrap();
    assert!(error.contains("checkpoint not found for this range"));

    assert!(block_on(backend.get_ignore_patterns(repo.clone()))
        .unwrap()
        .is_empty());
    let patterns = vec!["*.lock".into(), "generated/**".into()];
    block_on(backend.set_ignore_patterns(repo.clone(), patterns.clone())).unwrap();
    assert_eq!(
        block_on(backend.get_ignore_patterns(repo.clone())).unwrap(),
        patterns
    );
    block_on(backend.set_ignore_patterns(repo.clone(), Vec::new())).unwrap();

    let threads = block_on(backend.list_threads(stack_id.clone(), true)).unwrap();
    assert_eq!(threads.len(), 4);
    assert!(threads.iter().enumerate().all(|(i, pr)| i == pr.pr_index));
    let seeded = threads[1].threads.first().expect("PR #2 seeded thread");
    assert_eq!(
        block_on(backend.list_threads(stack_id.clone(), false)).unwrap()[1].threads,
        threads[1].threads
    );
    let line = seeded.line.unwrap();
    assert!(block_on(backend.accepts_line_comment(
        stack_id.clone(),
        1,
        seeded.path.clone(),
        Side::Right,
        line,
        line
    ))
    .unwrap());
    let located = backend
        .locate_anchors(
            range.view_id.clone(),
            vec![Anchor {
                pr: 1,
                path: seeded.path.clone(),
                side: Side::Right,
                line,
            }],
        )
        .unwrap();
    assert!(located[0].is_some());
    let (offset, changed) = range
        .view
        .rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.n.is_some())
        .unwrap();
    // Use the file containing that row, since row offsets are global in the view.
    let file_index = range
        .view
        .summary
        .files
        .iter()
        .position(|f| f.first_row <= offset as u32 && (offset as u32) < f.first_row + f.row_count)
        .unwrap();
    assert!(backend
        .locate_line(range.view_id.clone(), file_index, changed.n.unwrap())
        .unwrap()
        .is_some());
    assert!(
        !block_on(backend.head_lines(range.view_id.clone(), file_index, 1, 3))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        block_on(backend.head_lines(range.view_id.clone(), usize::MAX, 1, 3)).unwrap_err(),
        "no such file"
    );

    let draft = block_on(backend.create_draft(
        stack_id.clone(),
        NewDraft {
            pr_index: 1,
            kind: DraftKind::Line,
            path: Some(seeded.path.clone()),
            side: Some(Side::Right),
            line: Some(line),
            start_line: None,
            body: "Please explain this change".into(),
            thread_id: None,
            reply_to: None,
            assistant: false,
        },
    ))
    .unwrap();
    let drafts = block_on(backend.list_drafts(stack_id.clone())).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].draft, draft);
    assert_eq!(drafts[0].pr_index, 1);
    let updated =
        block_on(backend.update_draft(draft.id.clone(), Some("Updated draft".into()), Some(true)))
            .unwrap()
            .unwrap();
    assert_eq!(updated.body, "Updated draft");
    assert!(updated.as_file);
    block_on(backend.delete_draft(draft.id.clone())).unwrap();
    assert!(block_on(backend.list_drafts(stack_id.clone()))
        .unwrap()
        .is_empty());
    assert!(block_on(backend.update_draft(draft.id, None, None))
        .unwrap()
        .is_none());

    let markdown =
        block_on(backend.save_image_data("image/png".into(), b"encoded image bytes".to_vec()))
            .unwrap();
    let image_id = wispy_core::images::references(&markdown).pop().unwrap();
    assert_eq!(
        block_on(backend.image_bytes(image_id.clone())).unwrap(),
        Some(("image/png".into(), b"encoded image bytes".to_vec()))
    );
    assert!(block_on(backend.image_data_url(image_id))
        .unwrap()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert!(block_on(backend.image_bytes("missing".into()))
        .unwrap()
        .is_none());
    let base64_markdown = block_on(backend.save_image("image/gif".into(), "R0lG".into())).unwrap();
    let base64_id = wispy_core::images::references(&base64_markdown)
        .pop()
        .unwrap();
    assert_eq!(
        block_on(backend.image_bytes(base64_id)).unwrap(),
        Some(("image/gif".into(), b"GIF".to_vec()))
    );
    assert!(block_on(backend.save_image_data("text/plain".into(), b"no".to_vec())).is_err());

    let usages = block_on(backend.usages(range.view_id.clone(), "Service0".into())).unwrap();
    assert!(!usages.definitions.is_empty());
    assert_eq!(usages.definitions[0].path, "src/Module0/Service0.php");
    let (grep_sender, grep_receiver) = mpsc::channel();
    block_on(
        backend.grep(stack_id.clone(), 3, "Service0".into(), true, move |event| {
            grep_sender.send(event).unwrap();
        }),
    )
    .unwrap();
    let events: Vec<_> = grep_receiver.try_iter().collect();
    let mut hit_count = 0;
    for event in &events {
        if let GrepEvent::Hits { hits } = event {
            assert!(!hits.is_empty());
            assert!(hits.iter().all(|hit| hit.text.contains("Service0")));
            hit_count += hits.len();
        }
    }
    assert!(hit_count > 0);
    assert!(
        matches!(events.last(), Some(GrepEvent::Done { total, unsearched: 0 }) if *total == hit_count)
    );
    let path = "src/Module0/Service0.php".to_string();
    let lines = block_on(backend.read_file(stack_id.clone(), 3, path.clone())).unwrap();
    assert!(lines
        .iter()
        .flatten()
        .any(|(_, text)| text.contains("Service0")));
    assert_eq!(
        block_on(backend.indent_unit(stack_id.clone(), 3, path.clone())).unwrap(),
        "    "
    );
    assert_eq!(
        block_on(backend.existing_files(
            stack_id.clone(),
            3,
            vec![path.clone(), "does-not-exist".into(), "src".into()]
        ))
        .unwrap(),
        [path]
    );

    assert!(block_on(backend.get_inbox()).unwrap().is_empty());
    let (inbox_sender, inbox_receiver) = mpsc::channel();
    let (status_sender, status_receiver) = mpsc::channel();
    backend.set_on_inbox(move |entries| {
        inbox_sender.send(entries).unwrap();
    });
    backend.set_on_inbox_status(move |online| {
        status_sender.send(online).unwrap();
    });
    backend.start_prefetch();
    backend.start_prefetch(); // idempotent
    assert!(status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap());
    loop {
        let entries = inbox_receiver
            .recv_timeout(Duration::from_secs(120))
            .unwrap();
        if !entries.is_empty() && entries.iter().all(|entry| entry.ready) {
            break;
        }
    }
    let inbox = block_on(backend.get_inbox()).unwrap();
    assert!(!inbox.is_empty());
    assert!(inbox.iter().all(|entry| entry.ready));
    assert!(inbox
        .iter()
        .flat_map(|entry| &entry.progress)
        .any(|progress| progress.marked));

    let draft = block_on(backend.create_draft(
        stack_id.clone(),
        NewDraft {
            pr_index: 1,
            kind: DraftKind::Line,
            path: Some(seeded.path.clone()),
            side: Some(Side::Right),
            line: Some(line),
            start_line: None,
            body: "Ready to submit".into(),
            thread_id: None,
            reply_to: None,
            assistant: false,
        },
    ))
    .unwrap();
    let plan = block_on(backend.prepare_submit(stack_id.clone())).unwrap();
    assert_eq!(plan.viewer, "you");
    assert_eq!(plan.stack.stack_id, stack_id);
    assert!(!plan.stack.from_cache);
    assert_eq!(plan.prs.len(), 1);
    assert_eq!(plan.prs[0].pr_index, 1);
    assert_eq!(plan.prs[0].planned[0].draft.id, draft.id);
    let outcome =
        block_on(backend.submit_review(plan.stack.stack_id, 1, Verdict::Comment, None)).unwrap();
    assert_eq!(outcome.posted, 1);
    assert!(outcome.failed.is_empty());
    assert_eq!(outcome.unknown, 0);
    assert!(block_on(backend.list_drafts(stack_id.clone()))
        .unwrap()
        .is_empty());
    assert!(block_on(backend.list_checkpoints(stack_id.clone(), 1, 1))
        .unwrap()
        .iter()
        .any(|c| c.source == "submit"));
    // Submitting triggers a refresh; no explicit trigger here.
    assert!(status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap());
    loop {
        let entries = inbox_receiver
            .recv_timeout(Duration::from_secs(120))
            .unwrap();
        if entries.iter().any(|entry| {
            entry
                .progress
                .iter()
                .any(|progress| progress.review.as_deref() == Some("COMMENTED"))
        }) {
            break;
        }
    }

    let merge = block_on(backend.merge_info(stack_id.clone(), 1)).unwrap();
    assert_eq!(merge.base_ref, opened.prs[0].head_ref);
    assert!(!merge.ready());
    assert_eq!(merge.methods[0], MergeMethod::Squash);
    let merged = block_on(backend.merge_pr(
        stack_id.clone(),
        1,
        MergeRequest {
            method: merge.methods[0],
            title: merge.title,
            body: merge.body,
        },
    ))
    .unwrap();
    assert_eq!(merged, MergeOutcome::AutoMergeEnabled);
    assert!(status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap());
    assert!(
        block_on(backend.merge_info(stack_id.clone(), 1))
            .unwrap()
            .auto_merge_enabled
    );
    backend.trigger_prefetch();
    assert!(status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap());
    backend.refresh_inbox();
    assert!(status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap());

    let (assistant_sender, assistant_receiver) = mpsc::channel();
    let thread = block_on(backend.ask_assistant(
        stack_id.clone(),
        1,
        1,
        None,
        Some(NewThread {
            provider: Provider::Claude,
            model: None,
            effort: None,
            selection: None,
            anchor: None,
            hidden: Vec::new(),
        }),
        None,
        "say:backend round trip".into(),
        move |event| {
            assistant_sender.send(event).unwrap();
        },
    ))
    .unwrap();
    assert_eq!(thread.session.as_deref(), Some("fake-claude-session"));
    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.messages[1].text, "backend round trip");
    let events: Vec<_> = assistant_receiver.try_iter().collect();
    assert!(events
        .iter()
        .any(|event| matches!(event, AssistantEvent::Delta { .. })));
    assert!(events.iter().any(
        |event| matches!(event, AssistantEvent::Text { text } if text == "backend round trip")
    ));
    assert_eq!(
        block_on(backend.list_assistant_threads(stack_id.clone())).unwrap(),
        [thread.clone()]
    );
    let follow_up = block_on(backend.ask_assistant(
        stack_id.clone(),
        1,
        1,
        Some(thread.id.clone()),
        None,
        None,
        "follow up".into(),
        |_| {},
    ))
    .unwrap();
    assert_eq!(follow_up.messages.len(), 4);
    assert!(follow_up.messages[3].text.starts_with("(follow-up)"));
    block_on(backend.delete_assistant_thread(thread.id)).unwrap();
    assert!(block_on(backend.list_assistant_threads(stack_id.clone()))
        .unwrap()
        .is_empty());
    let (url_sender, url_receiver) = mpsc::channel();
    block_on(backend.sign_in_assistant(Provider::Claude, move |url| {
        url_sender.send(url).unwrap();
    }))
    .unwrap();
    let url = url_receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        url,
        "https://claude.example.test/oauth/authorize?state=fake"
    );
    let log = std::fs::read_to_string(fixture.root.join("assistant.jsonl")).unwrap();
    assert!(log
        .lines()
        .any(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["opened"] == url));
    assert_eq!(
        backend.open_url("file:///tmp/private".into()).unwrap_err(),
        "not a web link: file:///tmp/private"
    );
    backend.prefs().remember_pr("wispy/fixture#1");
    assert_eq!(backend.prefs().recent_prs(), ["wispy/fixture#1"]);

    backend
        .lasting()
        .set("answer", "shared".to_string(), String::new());
    assert_eq!(
        backend.clone().lasting().get("answer", String::new()),
        "shared"
    );

    // Offline refresh preserves the cached inbox and reports connectivity through its callback.
    let mut server = fixture.server.take().unwrap();
    server.kill().unwrap();
    server.wait().unwrap();
    backend.refresh_inbox();
    while status_receiver
        .recv_timeout(Duration::from_secs(120))
        .unwrap()
    {}
    assert!(!block_on(backend.get_inbox()).unwrap().is_empty());

    // Let precompute/index threads release their Arcs before removing their data directory.
    let weak = Arc::downgrade(&backend);
    drop(backend);
    let deadline = Instant::now() + Duration::from_secs(120);
    while weak.strong_count() != 0 {
        assert!(Instant::now() < deadline, "backend workers did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Starts the fake GitHub server on a free port it picks itself (`--port 0`) and returns the
/// port it printed once listening: no window where another test's server can take the port.
fn spawn_fixture_server(mut command: Command) -> (std::process::Child, u16) {
    use std::io::BufRead;
    let mut server = command
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .strip_prefix("port ")
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| {
            let _ = server.kill();
            panic!("fixture server did not start: {line:?}")
        });
    (server, port)
}
