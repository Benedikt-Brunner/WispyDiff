//! The tools the assistant works on the review's drafts with, served to its CLI over MCP during
//! a turn: add a draft comment on head lines, list the drafts by ref, edit or delete one.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::assistant::{draft_label, draft_listing, draft_ref, CommentAction, CommentOutcome, DraftRef};
use crate::drafts::{DraftKind, DraftStatus};
use crate::github::Side;
use crate::mcp::Tool;
use crate::model::DiffView;
use crate::service::{NewDraft, PrService};
use crate::stack::StackSnapshot;

/// One turn's tools on stack PRs `lo..=hi`. `refs` are the thread's draft refs; `on_outcome`
/// hears of every change as it happens.
pub(crate) struct ReviewTools<'a> {
    service: &'a PrService,
    snapshot: &'a StackSnapshot,
    range: (usize, usize),
    /// The checkout's path with a trailing slash (Codex sometimes names files by absolute path).
    checkout: String,
    view: OnceLock<Arc<DiffView>>,
    state: Mutex<(Vec<DraftRef>, Vec<CommentOutcome>)>,
    on_outcome: &'a (dyn Fn(&CommentOutcome) + Sync),
}

impl<'a> ReviewTools<'a> {
    pub(crate) fn new(
        service: &'a PrService,
        snapshot: &'a StackSnapshot,
        range: (usize, usize),
        checkout: &std::path::Path,
        refs: Vec<DraftRef>,
        on_outcome: &'a (dyn Fn(&CommentOutcome) + Sync),
    ) -> Self {
        let checkout = format!("{}/", checkout.to_string_lossy());
        ReviewTools { service, snapshot, range, checkout, view: OnceLock::new(), state: Mutex::new((refs, Vec::new())), on_outcome }
    }

    /// The thread's refs and what the turn did.
    pub(crate) fn finish(self) -> (Vec<DraftRef>, Vec<CommentOutcome>) {
        self.state.into_inner().unwrap_or_else(|p| p.into_inner())
    }

    fn add(&self, args: &Value) -> Result<String, String> {
        let path = string(args, "path")?;
        let body = string(args, "body")?;
        let path = path.strip_prefix(&self.checkout).unwrap_or(&path).trim_start_matches("./").to_string();
        let line = |key| args[key].as_u64().map(|n| n as u32);
        let lines = match (line("start_line"), line("end_line")) {
            (Some(start), Some(end)) => Some((start, end)),
            (Some(line), None) | (None, Some(line)) => Some((line, line)),
            (None, None) => None,
        };
        let mut outcome = CommentOutcome { action: CommentAction::Add, path: Some(path.clone()), lines, target: None, label: None, body: Some(body.clone()), draft: None, error: None };
        let placed = match lines {
            Some((start, end)) if start == 0 || start > end => Err(format!("{start}–{end} isn't a line range")),
            _ => self.view().and_then(|view| crate::anchors::head_target(&view, &path, lines)),
        };
        let draft = placed.and_then(|target| {
            let new = NewDraft {
                pr_index: target.pr as usize,
                kind: if target.line.is_some() { DraftKind::Line } else { DraftKind::File },
                path: Some(target.path),
                side: target.line.map(|_| Side::Right),
                line: target.line,
                start_line: target.start_line,
                body: body.clone(),
                thread_id: None,
                reply_to: None,
                assistant: true,
            };
            self.service.create_draft(self.snapshot, new).map_err(|e| e.to_string())
        });
        let result = match draft {
            Ok(draft) => {
                let mut state = self.lock();
                let name = draft_ref(&mut state.0, &draft.id, &body);
                let label = draft_label(draft.kind, draft.path.as_deref(), draft.line.map(|end| (draft.start_line.unwrap_or(end), end)), draft.pr);
                let text = format!("Added draft {name} on {label}.");
                (outcome.target, outcome.label, outcome.draft) = (Some(name), Some(label), Some(draft.id));
                Ok(text)
            }
            Err(why) => {
                outcome.error = Some(why.clone());
                Err(why)
            }
        };
        self.record(outcome);
        result
    }

    fn list(&self) -> Result<String, String> {
        let (lo, hi) = self.range;
        let drafts = self.service.listed_drafts(self.snapshot, lo, hi).map_err(|e| e.to_string())?;
        Ok(draft_listing(&mut self.lock().0, &drafts))
    }

    /// Edits (`body`) or deletes the draft the assistant knows as `name`, unless the reviewer
    /// changed it since the assistant last saw it.
    fn change(&self, action: CommentAction, args: &Value) -> Result<String, String> {
        let name = string(args, "ref")?;
        let body = match action {
            CommentAction::Edit => Some(string(args, "body")?),
            _ => None,
        };
        let mut outcome = CommentOutcome { action, path: None, lines: None, target: Some(name.clone()), label: None, body: body.clone(), draft: None, error: None };
        let result = self.apply_change(&name, body, &mut outcome);
        self.record(CommentOutcome { error: result.as_ref().err().map(|(why, _)| why.clone()), ..outcome });
        // The agent also hears how to go on.
        result.map_err(|(why, hint)| format!("{why}{hint}"))
    }

    /// Err: why (for the reviewer too) and a hint for the agent.
    fn apply_change(&self, name: &str, body: Option<String>, outcome: &mut CommentOutcome) -> Result<String, (String, &'static str)> {
        let mut state = self.lock();
        let Some(known) = state.0.iter_mut().find(|r| r.name == name) else {
            return Err((format!("there's no draft {name}"), " (list_drafts shows the drafts by ref)"));
        };
        let draft = self.service.draft(&known.draft).map_err(|e| (e.to_string(), ""))?;
        let Some(draft) = draft.filter(|d| matches!(d.status, DraftStatus::Draft | DraftStatus::Failed)) else {
            return Err((format!("{name} is no longer a draft"), ""));
        };
        outcome.label = Some(draft_label(draft.kind, draft.path.as_deref(), draft.line.map(|end| (draft.start_line.unwrap_or(end), end)), draft.pr));
        if known.seen.as_deref() != Some(draft.body.as_str()) {
            return Err((format!("the reviewer changed {name} since you last saw it"), " (list_drafts shows its text now)"));
        }
        let done = match body {
            Some(body) => {
                self.service.update_draft(&draft.id, Some(body.clone()), None).map_err(|e| (e.to_string(), ""))?;
                known.seen = Some(body);
                format!("Updated {name}.")
            }
            None => {
                self.service.delete_draft(&draft.id).map_err(|e| (e.to_string(), ""))?;
                known.seen = None;
                format!("Deleted {name}.")
            }
        };
        outcome.draft = Some(draft.id);
        Ok(done)
    }

    fn view(&self) -> Result<Arc<DiffView>, String> {
        if let Some(view) = self.view.get() {
            return Ok(view.clone());
        }
        let (lo, hi) = self.range;
        let view = self.service.range(self.snapshot, lo, hi).map_err(|e| e.to_string())?;
        Ok(self.view.get_or_init(|| view).clone())
    }

    fn record(&self, outcome: CommentOutcome) {
        (self.on_outcome)(&outcome);
        self.lock().1.push(outcome);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<DraftRef>, Vec<CommentOutcome>)> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl crate::mcp::Tools for ReviewTools<'_> {
    fn list(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: "add_draft_comment",
                description: "Adds a draft review comment on lines of the checkout (the head of the PRs under review), \
                    or on a whole file. It goes to the PR that changed those lines, like the reviewer's own comments. \
                    Only files changed in the diff under review can be commented on, and only lines that exist at the \
                    head (not removed ones). Returns the new draft's ref.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Repo-relative path." },
                        "start_line": { "type": "integer", "minimum": 1, "description": "First commented line in the checkout; leave out start_line and end_line to comment on the whole file." },
                        "end_line": { "type": "integer", "minimum": 1, "description": "Last commented line (defaults to start_line)." },
                        "body": { "type": "string", "description": "The comment, GitHub Markdown. A ```suggestion block replaces exactly the commented lines." },
                    },
                    "required": ["path", "body"],
                }),
            },
            Tool {
                name: "list_drafts",
                description: "Lists the review's pending draft comments (the reviewer's and yours) by ref, with where they are \
                    and their text. List them before changing drafts you didn't just write.",
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            Tool {
                name: "edit_draft",
                description: "Replaces the text of a draft comment, by ref. Refused if the reviewer changed it since you last saw it.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "ref": { "type": "string", "description": "e.g. \"d3\"" },
                        "body": { "type": "string", "description": "The complete new text, GitHub Markdown." },
                    },
                    "required": ["ref", "body"],
                }),
            },
            Tool {
                name: "delete_draft",
                description: "Deletes a draft comment, by ref. Refused if the reviewer changed it since you last saw it.",
                input_schema: json!({
                    "type": "object",
                    "properties": { "ref": { "type": "string", "description": "e.g. \"d3\"" } },
                    "required": ["ref"],
                }),
            },
        ]
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "add_draft_comment" => self.add(args),
            "list_drafts" => self.list(),
            "edit_draft" => self.change(CommentAction::Edit, args),
            "delete_draft" => self.change(CommentAction::Delete, args),
            _ => Err(format!("there's no tool {name}")),
        }
    }
}

fn string(args: &Value, key: &str) -> Result<String, String> {
    args[key].as_str().map(str::to_string).ok_or_else(|| format!("`{key}` is missing"))
}
