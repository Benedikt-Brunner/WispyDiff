//! Its own test binary: it points `WISPY_CLAUDE_BIN` (process-wide) at a missing CLI.

use wispy_core::assistant::{run, Ask, Provider};

#[test]
fn a_missing_cli_is_reported_by_name() {
    unsafe { std::env::set_var("WISPY_CLAUDE_BIN", "/nonexistent/claude") };
    let ask = Ask { provider: Provider::Claude, model: None, effort: None, prompt: "q".into(), resume: None };
    let err = run(&ask, std::path::Path::new("/tmp"), |_| {}).unwrap_err().to_string();
    assert!(err.contains("`/nonexistent/claude` not found"), "{err}");
}
