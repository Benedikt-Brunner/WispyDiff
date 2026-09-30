//! Apps launched from Finder get a minimal PATH; adopt the login shell's so `git`,
//! `gh`, `claude` and `codex` resolve the same way they do in the terminal.

use std::process::Command;

const MARKER: &str = "__WISPY_PATH__";

pub fn adopt_login_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");
    let Ok(output) = Command::new(shell).args(["-l", "-c", &script]).output() else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(path) = stdout.split(MARKER).nth(1).filter(|p| !p.is_empty()) {
        std::env::set_var("PATH", path);
    }
}
