//! Apps launched from Finder (or a desktop launcher) get a minimal PATH; the user's shell
//! knows the real one, so `git`, `gh`, `claude` and `codex` resolve as in the terminal.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

const MARKER: &str = "__WISPY_PATH__";

/// The login shell's PATH. Cheap enough for startup, but zsh skips `.zshrc` here.
pub fn login_shell_path() -> Option<String> {
    shell_path(&["-l", "-c"])
}

/// The interactive login shell's PATH (also sources `.zshrc` / `.bashrc`, where
/// e.g. `~/.local/bin` is often added). Slow (a second or more), so resolved once, on demand.
pub fn interactive_shell_path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| shell_path(&["-i", "-l", "-c"])).as_deref()
}

fn shell_path(flags: &[&str]) -> Option<String> {
    let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" };
    let shell = std::env::var("SHELL").unwrap_or_else(|_| fallback.to_string());
    let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");
    let output = Command::new(shell)
        .args(flags)
        .arg(&script)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.split(MARKER).nth(1).filter(|p| !p.is_empty()).map(str::to_string)
}
