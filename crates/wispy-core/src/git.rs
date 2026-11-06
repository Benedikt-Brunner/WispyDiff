use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;

use crate::error::{Error, Result};

/// Maps file types to git's built-in function-name patterns (`userdiff`), plus `wispy-ts`
/// for JavaScript/TypeScript, which git has none for.
const DIFF_ATTRIBUTES: &str = "\
*.php diff=php
*.phtml diff=php
*.rs diff=rust
*.py diff=python
*.go diff=golang
*.java diff=java
*.kt diff=kotlin
*.kts diff=kotlin
*.rb diff=ruby
*.cs diff=csharp
*.c diff=cpp
*.h diff=cpp
*.cc diff=cpp
*.cpp diff=cpp
*.cxx diff=cpp
*.hh diff=cpp
*.hpp diff=cpp
*.mm diff=objc
*.css diff=css
*.scss diff=css
*.html diff=html
*.md diff=markdown
*.sh diff=bash
*.bash diff=bash
*.pl diff=perl
*.pm diff=perl
*.ex diff=elixir
*.exs diff=elixir
*.js diff=wispy-ts
*.jsx diff=wispy-ts
*.mjs diff=wispy-ts
*.cjs diff=wispy-ts
*.ts diff=wispy-ts
*.tsx diff=wispy-ts
*.mts diff=wispy-ts
*.cts diff=wispy-ts
*.vue diff=wispy-ts
";

/// One pattern per line, tried in order; a `!` line rejects matches (control-flow blocks).
/// Matches classes, functions, arrow functions bound to a name, and methods.
const TS_FUNCNAME: &str = concat!(
    "!^[\t ]*(if|else|for|while|do|switch|catch|try|return|with)([^A-Za-z0-9_$]|$)\n",
    "^[\t ]*((export[\t ]+)?(default[\t ]+)?(abstract[\t ]+)?(class|interface|enum|namespace)[\t ]+[A-Za-z_$].*)$\n",
    "^[\t ]*((export[\t ]+)?(default[\t ]+)?(async[\t ]+)?function[\t *]*[A-Za-z_$]?.*)$\n",
    "^[\t ]*((export[\t ]+)?(const|let|var)[\t ]+[A-Za-z_$][A-Za-z0-9_$]*[\t ]*(:[^=]*)?=[\t ]*(async[\t ]+)?",
    "(function|\\(|[A-Za-z_$][A-Za-z0-9_$]*[\t ]*=>).*)$\n",
    "^[\t ]*(((public|private|protected|static|readonly|override|abstract|async|get|set)[\t ]+)*",
    "[A-Za-z_$#][A-Za-z0-9_$]*[\t ]*(<[^>]*>)?\\(.*\\)[^;=]*\\{[\t ]*)$",
);

/// Runs git against one bare repository, isolated from the user's global and system
/// config (so `insteadOf` rewrites or credential helpers can't interfere).
#[derive(Debug, Clone)]
pub struct Git {
    git_dir: PathBuf,
    github_token: Option<String>,
}

impl Git {
    pub fn new(git_dir: impl Into<PathBuf>, github_token: Option<String>) -> Self {
        Git { git_dir: git_dir.into(), github_token }
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Makes hunk headers (`@@ … @@ <section>`) name the enclosing function or method rather
    /// than git's default: the nearest unindented line, usually the class.
    pub fn write_diff_attributes(&self) -> Result<()> {
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info)?;
        std::fs::write(info.join("attributes"), DIFF_ATTRIBUTES)?;
        Ok(())
    }

    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.command(args).output()?;
        Self::check(args, output.status.success(), output.stdout, &output.stderr)
    }

    /// Runs git and returns whether it succeeded plus stdout, for commands whose non-zero exit
    /// is an answer rather than a failure (e.g. `merge-tree` reporting conflicts).
    pub fn run_status(&self, args: &[&str]) -> Result<(bool, Vec<u8>)> {
        let output = self.command(args).output()?;
        Ok((output.status.success(), output.stdout))
    }

    /// Starts git with stdout piped, for streaming output (e.g. `grep`).
    pub fn spawn(&self, args: &[&str]) -> Result<std::process::Child> {
        Ok(self.command(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?)
    }

    /// Like [`Git::spawn`], but objects missing from the partial clone are never downloaded one
    /// by one. (Newer git then fails on them rather than skipping, so pass only downloaded paths.)
    pub fn spawn_local(&self, args: &[&str]) -> Result<std::process::Child> {
        Ok(self.command(args).env("GIT_NO_LAZY_FETCH", "1").stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?)
    }

    pub fn run_string(&self, args: &[&str]) -> Result<String> {
        let out = self.run(args)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    pub fn run_with_stdin(&self, args: &[&str], stdin: &[u8]) -> Result<Vec<u8>> {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut pipe = child.stdin.take().expect("stdin is piped");
        let input = stdin.to_vec();
        let writer = std::thread::spawn(move || pipe.write_all(&input));
        let output = child.wait_with_output()?;
        writer.join().expect("stdin writer panicked")?;
        Self::check(args, output.status.success(), output.stdout, &output.stderr)
    }

    fn check(args: &[&str], ok: bool, stdout: Vec<u8>, stderr: &[u8]) -> Result<Vec<u8>> {
        if ok {
            Ok(stdout)
        } else {
            Err(Error::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(stderr).trim().to_string(),
            })
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("--git-dir").arg(&self.git_dir).args(args);
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("LC_ALL", "C");

        let mut config = vec![
            ("core.quotepath".to_string(), "false".to_string()),
            ("protocol.file.allow".to_string(), "always".to_string()),
            ("diff.wispy-ts.xfuncname".to_string(), TS_FUNCNAME.to_string()),
        ];
        if let Some(token) = &self.github_token {
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
            config.push((
                "http.https://github.com/.extraheader".to_string(),
                format!("AUTHORIZATION: basic {basic}"),
            ));
        }
        cmd.env("GIT_CONFIG_COUNT", config.len().to_string());
        for (i, (key, value)) in config.into_iter().enumerate() {
            cmd.env(format!("GIT_CONFIG_KEY_{i}"), key).env(format!("GIT_CONFIG_VALUE_{i}"), value);
        }
        cmd
    }
}
