use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;

use crate::error::{Error, Result};

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

    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.command(args).output()?;
        Self::check(args, output.status.success(), output.stdout, &output.stderr)
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
