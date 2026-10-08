mod support;

use std::path::PathBuf;

use support::{pull_request, OriginRepo};
use wispy_core::assistant::{command, first_prompt, needs_sign_in, parse_line, sign_in, sign_in_command, sign_in_url, Ask, AssistantEvent, Context, Provider, Selection};
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::{NewThread, PrService};
use wispy_core::stack::Stack;

fn fake(name: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../e2e/fake-cli").join(name);
    root.canonicalize().unwrap().to_string_lossy().into_owned()
}

/// The questions the fake CLIs were asked (the sign-in test runs alongside and logs there too).
fn asks(log: &std::path::Path) -> Vec<serde_json::Value> {
    let entries = std::fs::read_to_string(log).unwrap();
    entries.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).filter(|c| c.get("prompt").is_some()).collect()
}

fn args(ask: &Ask) -> Vec<String> {
    let cmd = command(ask, std::path::Path::new("/tmp"));
    cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
}

fn ask(provider: Provider, resume: Option<&str>) -> Ask {
    Ask { provider, model: Some("m1".into()), effort: Some("high".into()), prompt: "q".into(), resume: resume.map(str::to_string), tools: None }
}

#[test]
fn builds_read_only_claude_invocations() {
    let first = args(&ask(Provider::Claude, None));
    let joined = first.join(" ");
    assert!(joined.starts_with("-p --output-format stream-json --verbose --include-partial-messages"));
    assert!(joined.contains("--tools Read,Grep,Glob --permission-mode bypassPermissions"), "{joined}");
    assert!(!joined.contains("--strict-mcp-config"), "the user's own MCP servers stay available");
    assert!(joined.contains("--model m1") && joined.contains("--effort high"));
    assert!(!joined.contains("--resume"));
    assert!(args(&ask(Provider::Claude, Some("s-1"))).join(" ").ends_with("--resume s-1"));
    let defaults = Ask { model: None, effort: None, ..ask(Provider::Claude, None) };
    assert!(!args(&defaults).iter().any(|a| a == "--model" || a == "--effort"));

    // The review tools: the app's MCP server, next to the user's own.
    let tools = Ask { tools: Some(endpoint()), ..ask(Provider::Claude, None) };
    let with_tools = args(&tools);
    let config = with_tools.windows(2).find(|w| w[0] == "--mcp-config").map(|w| w[1].clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&config).unwrap(),
        serde_json::json!({ "mcpServers": { "wispy": { "type": "http", "url": "http://127.0.0.1:4711/mcp", "headers": { "Authorization": "Bearer s3cret" } } } })
    );
}

fn endpoint() -> wispy_core::mcp::Endpoint {
    wispy_core::mcp::Endpoint { url: "http://127.0.0.1:4711/mcp".into(), token: "s3cret".into() }
}

#[test]
fn builds_read_only_codex_invocations() {
    assert_eq!(
        args(&ask(Provider::Codex, None)),
        ["exec", "--json", "--skip-git-repo-check", "-c", "sandbox_mode=\"read-only\"", "-m", "m1", "-c", "model_reasoning_effort=\"high\"", "-"]
    );
    assert_eq!(&args(&ask(Provider::Codex, Some("t-1")))[..3], ["exec", "resume", "t-1"]);

    // The review tools: approved up front (exec can't ask), the token passed in the environment.
    let tools = Ask { tools: Some(endpoint()), ..ask(Provider::Codex, None) };
    let cmd = command(&tools, std::path::Path::new("/tmp"));
    let with_tools: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert!(with_tools.contains(&"mcp_servers.wispy={url=\"http://127.0.0.1:4711/mcp\", bearer_token_env_var=\"WISPY_MCP_TOKEN\", default_tools_approval_mode=\"approve\", required=true}".to_string()), "{with_tools:?}");
    assert!(!with_tools.iter().any(|a| a.contains("s3cret")));
    assert!(cmd.get_envs().any(|(k, v)| k == "WISPY_MCP_TOKEN" && v == Some(std::ffi::OsStr::new("s3cret"))));
}

#[test]
fn parses_both_clis_event_streams() {
    let claude = |line: &str| parse_line(Provider::Claude, line);
    assert_eq!(claude(r#"{"type":"system","subtype":"init","session_id":"abc"}"#), vec![AssistantEvent::Session { id: "abc".into() }]);
    assert_eq!(
        claude(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#),
        vec![AssistantEvent::Delta { text: "Hel".into() }]
    );
    assert_eq!(
        claude(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"},{"type":"tool_use","name":"Read"}]}}"#),
        vec![AssistantEvent::Text { text: "Hello".into() }]
    );
    assert_eq!(
        claude(r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate: OAuth session expired"}"#),
        vec![AssistantEvent::Error { message: "Failed to authenticate: OAuth session expired".into() }]
    );
    assert!(claude(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#).is_empty());
    assert!(claude("not json").is_empty());

    let codex = |line: &str| parse_line(Provider::Codex, line);
    assert_eq!(codex(r#"{"type":"thread.started","thread_id":"t-9"}"#), vec![AssistantEvent::Session { id: "t-9".into() }]);
    assert_eq!(
        codex(r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Done"}}"#),
        vec![AssistantEvent::Text { text: "Done".into() }]
    );
    assert!(codex(r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Model metadata not found"}}"#).is_empty(), "warnings aren't failures");
    let failed = r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'x' model requires a newer version of Codex.\"}}"}}"#;
    assert_eq!(codex(failed), vec![AssistantEvent::Error { message: "The 'x' model requires a newer version of Codex.".into() }]);
}

#[test]
fn asks_in_a_read_only_checkout_of_the_head_and_follows_up_in_the_same_session() {
    // SAFETY: this test binary's tests don't read these variables concurrently in other ways.
    unsafe {
        std::env::set_var("WISPY_CLAUDE_BIN", fake("claude"));
        std::env::set_var("WISPY_CODEX_BIN", fake("codex"));
    }
    let log = tempfile::NamedTempFile::new().unwrap();
    unsafe { std::env::set_var("WISPY_FAKE_LOG", log.path()) };

    let origin = OriginRepo::init();
    origin.write("README.md", "# Shop\n");
    origin.write("src/Order.php", "<?php\n$state = 'open';\n");
    origin.write("src/composer.lock", "{\"hash\": \"lock-one\"}\n");
    origin.write("src/Old.lock", "alpha\nbeta\ngamma\ndelta\nold-lock-line\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("src/Order.php", "<?php\n$state = 'picked';\n");
    origin.write("src/composer.lock", "{\"hash\": \"lock-two\"}\n");
    origin.remove("src/Old.lock");
    origin.write("src/New.lock", "alpha\nbeta\ngamma\ndelta\nnew-lock-line\n");
    origin.commit("pick");
    origin.publish_pr(3, "feature");
    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let snapshot = service.fetch(Stack { prs: vec![pull_request(3, "main", "feature", &origin.url())], focus: 0 }).unwrap();

    let selection = Selection { path: "src/Order.php".into(), pr_label: "#3".into(), start_line: 2, end_line: 2, text: "+$state = 'picked';".into(), head_start: Some(2), head_end: Some(2) };
    let mut streamed = String::new();
    let new = NewThread { provider: Provider::Claude, model: Some("opus".into()), effort: Some("high".into()), selection: Some(selection), anchor: None, hidden: vec![] };
    let thread = service
        .ask(&snapshot, (0, 0), None, Some(new), "Is 'picked' a valid state?", |event| {
            if let AssistantEvent::Delta { text } = event {
                streamed.push_str(text);
            }
        })
        .unwrap();

    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.messages[0].text, "Is 'picked' a valid state?");
    let answer = &thread.messages[1];
    assert!(!answer.error);
    assert!(answer.text.contains("README.md, src"), "ran in a checkout of the head: {}", answer.text);
    assert_eq!(streamed.trim(), answer.text);
    assert_eq!(thread.session.as_deref(), Some("fake-claude-session"));

    let calls = asks(log.path());
    let prompt = calls[0]["prompt"].as_str().unwrap();
    assert!(prompt.contains("Under review: acme/shop #3"));
    assert!(prompt.contains("+$state = 'picked';"), "the diff and the selection are in the first prompt");
    assert!(prompt.contains("lines of src/Order.php (in #3, lines 2–2)"), "the checkout's numbers are only added when they differ");
    assert!(prompt.contains("like `src/app.ts:42`"), "asks for clickable references");
    assert!(prompt.contains("add each one with `add_draft_comment`"), "says how to write review comments");
    assert!(prompt.contains("lock-two") && prompt.contains("new-lock-line") && !prompt.contains("hid these"), "nothing hidden: {prompt}");
    let args: Vec<&str> = calls[0]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(args.windows(2).any(|w| w == ["--model", "opus"]));
    let cwd = PathBuf::from(calls[0]["cwd"].as_str().unwrap());
    assert!(cwd.starts_with(data.path().canonicalize().unwrap().join("worktrees/acme/shop")), "{cwd:?}");
    assert_eq!(std::fs::read_to_string(cwd.join("src/Order.php")).unwrap(), "<?php\n$state = 'picked';\n");

    // A follow-up resumes the session and sends only the question.
    let followed = service.ask(&snapshot, (0, 0), Some(&thread.id), None, "And 'shipped'?", |_| {}).unwrap();
    assert_eq!(followed.messages.len(), 4);
    assert!(followed.messages[3].text.starts_with("(follow-up)"));
    let calls = asks(log.path());
    assert_eq!(calls[1]["prompt"], "And 'shipped'?");
    assert!(calls[1]["args"].as_array().unwrap().iter().any(|a| a == "--resume"));

    // Failures are kept in the thread; Codex works the same way.
    let failed = service.ask(&snapshot, (0, 0), Some(&thread.id), None, "fail", |_| {}).unwrap();
    assert!(failed.messages.last().unwrap().error);
    assert!(failed.messages.last().unwrap().text.contains("Something went wrong (fake)"));
    assert!(!failed.messages.last().unwrap().sign_in);
    let expired = service.ask(&snapshot, (0, 0), Some(&thread.id), None, "expired", |_| {}).unwrap();
    assert!(expired.messages.last().unwrap().sign_in, "an expired sign-in offers to sign in again");

    // A thread whose first turn failed sends the review context again when asked again.
    let new = NewThread { provider: Provider::Claude, model: None, effort: None, selection: None, anchor: None, hidden: vec![] };
    let first = service.ask(&snapshot, (0, 0), None, Some(new), "expired", |_| {}).unwrap();
    assert_eq!(first.session, None);
    let retried = service.ask(&snapshot, (0, 0), Some(&first.id), None, "What changed?", |_| {}).unwrap();
    assert!(!retried.messages.last().unwrap().error);
    assert_eq!(retried.session.as_deref(), Some("fake-claude-session"));
    assert!(retried.unsent_context.is_none());
    let calls = asks(log.path());
    let call = calls.last().unwrap();
    assert!(call["prompt"].as_str().unwrap().contains("Under review: acme/shop #3"));
    assert!(!call["args"].as_array().unwrap().iter().any(|a| a == "--resume"));
    service.delete_assistant_thread(&first.id).unwrap();
    // Files hidden by the file filter are named, but their changes stay out of the prompt (a
    // hidden rename's old side too).
    let hidden = vec!["src/composer.lock".to_string(), "src/New.lock".to_string()];
    let new = NewThread { provider: Provider::Codex, model: None, effort: None, selection: None, anchor: None, hidden };
    let codex = service.ask(&snapshot, (0, 0), None, Some(new), "Summarize", |_| {}).unwrap();
    assert_eq!(codex.session.as_deref(), Some("fake-codex-thread"));
    assert!(codex.messages[1].text.contains("About “Summarize”"));
    let calls = asks(log.path());
    let prompt = calls.last().unwrap()["prompt"].as_str().unwrap();
    assert!(prompt.contains("hid these changed files") && prompt.contains("- src/composer.lock\n- src/New.lock\n"), "{prompt}");
    assert!(prompt.contains("+$state = 'picked';"), "the rest of the diff is still there");
    for hidden in ["lock-one", "lock-two", "old-lock-line", "new-lock-line"] {
        assert!(!prompt.contains(hidden), "{hidden} leaked into the prompt: {prompt}");
    }

    assert_eq!(service.assistant_threads(&snapshot).unwrap().len(), 2);
    service.delete_assistant_thread(&thread.id).unwrap();
    assert_eq!(service.assistant_threads(&snapshot).unwrap().len(), 1);
}

#[test]
fn names_the_checkouts_line_numbers_when_a_higher_pr_moved_the_selection() {
    let selection = Selection { path: "src/Order.php".into(), pr_label: "#2".into(), start_line: 10, end_line: 12, text: "x".into(), head_start: Some(14), head_end: Some(16) };
    let context = Context { range_label: "acme/shop #2–#3".into(), titles: vec![], diff: String::new(), selection: Some(selection), hidden: vec![] };
    assert!(first_prompt(&context, "Why?").contains("lines of src/Order.php (in #2, lines 10–12; lines 14–16 in the checkout)"));
}

#[test]
fn recognizes_expired_sign_ins() {
    assert!(needs_sign_in(Provider::Claude, "Failed to authenticate: OAuth session expired and could not be refreshed"));
    assert!(needs_sign_in(Provider::Claude, "Not logged in · Please run /login"));
    assert!(!needs_sign_in(Provider::Claude, "claude exited with exit status: 1: rate limited"));
    assert!(needs_sign_in(Provider::Codex, "unexpected status 401 Unauthorized"));
    assert!(!needs_sign_in(Provider::Codex, "model not available (fake)"));
}

#[test]
fn signs_in_without_letting_the_cli_open_a_browser() {
    let claude = sign_in_command(Provider::Claude);
    assert_eq!(claude.get_args().collect::<Vec<_>>(), ["auth", "login"]);
    assert!(claude.get_envs().any(|(k, v)| k == "BROWSER" && v == Some("true".as_ref())));
    assert_eq!(sign_in_command(Provider::Codex).get_args().collect::<Vec<_>>(), ["login"]);

    assert_eq!(
        sign_in_url("If the browser didn't open, visit: \x1b]8;;https://claude.ai/oauth?a=1\x07https://claude.ai/oauth?a=1\x1b]8;;\x07").as_deref(),
        Some("https://claude.ai/oauth?a=1")
    );
    assert_eq!(sign_in_url("Starting local login server on http://localhost:1455."), None);
}

#[test]
fn signs_in_through_the_cli_and_reports_the_sign_in_page() {
    // SAFETY: see above; both tests point at the same fakes.
    unsafe {
        std::env::set_var("WISPY_CLAUDE_BIN", fake("claude"));
        std::env::set_var("WISPY_CODEX_BIN", fake("codex"));
    }
    for (provider, expected) in [(Provider::Claude, "https://claude.example.test/"), (Provider::Codex, "https://codex.example.test/")] {
        let mut opened = None;
        sign_in(provider, |url| opened = Some(url.to_string())).unwrap();
        assert!(opened.as_deref().is_some_and(|u| u.starts_with(expected)), "{opened:?}");
    }
}
