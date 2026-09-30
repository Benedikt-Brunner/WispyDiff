//! Turning local drafts into one GitHub review per PR, without losing or double-posting anything.

use serde::Serialize;

use crate::anchors::{quoted_body, read_lines, remap_line, Coverage};
use crate::cache::Cache;
use crate::drafts::{new_id, now, Draft, DraftKind, DraftStatus};
use crate::error::{Error, Result};
use crate::git::Git;
use crate::github::{GitHubClient, NewReviewComment, Side, Verdict};
use crate::pr_ref::PrRef;

/// What submitting will do with a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Plan {
    /// A line comment at this position on the PR's current head.
    #[serde(rename_all = "camelCase")]
    Line { path: String, side: Side, line: u32, start_line: Option<u32> },
    /// A file comment; `quoted` when it stands in for a line comment (permalink + snippet).
    #[serde(rename_all = "camelCase")]
    File { path: String, quoted: bool },
    /// The commented lines changed since the draft was written: re-anchor, post as a file
    /// comment, or discard. Nothing is sent until the user decides.
    Outdated,
    Reply,
    Resolve,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Planned {
    pub draft: Draft,
    pub plan: Plan,
}

/// The PR as it is now on GitHub.
pub struct Target<'a> {
    pub pr: &'a PrRef,
    pub head: &'a str,
    /// Start of the PR's diff (GitHub's three-dot base).
    pub base: &'a str,
    pub coverage: &'a Coverage,
}

/// Decides, per pending draft, how it will be posted against the PR's current head.
pub fn plan(git: &Git, target: &Target, drafts: &[Draft]) -> Result<Vec<Planned>> {
    let mut planned = Vec::new();
    for draft in drafts.iter().filter(|d| d.is_pending()) {
        let plan = match draft.kind {
            DraftKind::Summary => Plan::Summary,
            DraftKind::Reply => Plan::Reply,
            DraftKind::Resolve => Plan::Resolve,
            DraftKind::File => Plan::File { path: draft.path.clone().unwrap_or_default(), quoted: false },
            DraftKind::Line => plan_line(git, target, draft)?,
        };
        planned.push(Planned { draft: draft.clone(), plan });
    }
    Ok(planned)
}

fn plan_line(git: &Git, target: &Target, draft: &Draft) -> Result<Plan> {
    let (Some(path), Some(side), Some(end)) = (&draft.path, draft.side, draft.line) else {
        return Ok(Plan::Outdated);
    };
    let start = draft.start_line.unwrap_or(end);
    let (from, to) = match side {
        Side::Right => (draft.commit.as_str(), target.head),
        Side::Left => (draft.base.as_str(), target.base),
    };
    let moved_end = remap_line(git, from, to, path, end)?;
    let moved_start = if start == end { moved_end.clone() } else { remap_line(git, from, to, path, start)? };
    match (moved_start, moved_end) {
        (Some((start_path, new_start)), Some((new_path, new_end))) if start_path == new_path && new_start <= new_end => {
            if target.coverage.accepts(&new_path, side, new_start, new_end) {
                Ok(Plan::Line { path: new_path, side, line: new_end, start_line: (new_start != new_end).then_some(new_start) })
            } else {
                Ok(Plan::File { path: new_path, quoted: true })
            }
        }
        _ if draft.as_file => Ok(Plan::File { path: path.clone(), quoted: true }),
        _ => Ok(Plan::Outdated),
    }
}

/// Everything to send for one PR, with bodies (quotes, markers) already built.
#[derive(Debug, Clone, Default)]
pub struct Payload {
    pub review_comments: Vec<(String, NewReviewComment)>,
    pub summary: Option<(Option<String>, String)>,
    pub file_comments: Vec<(String, String, String)>,
    pub replies: Vec<(String, u64, String)>,
    pub resolves: Vec<(String, String)>,
}

/// Builds the payload (reads quoted lines from git). `summary` overrides a stored summary draft.
pub fn payload(git: &Git, target: &Target, planned: &[Planned], summary: Option<&str>) -> Result<Payload> {
    let mut payload = Payload::default();
    let repo = format!("{}/{}", target.pr.owner, target.pr.repo);
    for Planned { draft, plan } in planned {
        match plan {
            Plan::Line { path, side, line, start_line } => payload.review_comments.push((
                draft.id.clone(),
                NewReviewComment {
                    path: path.clone(),
                    line: *line,
                    side: *side,
                    start_line: *start_line,
                    start_side: start_line.map(|_| *side),
                    body: draft.body.clone(),
                },
            )),
            Plan::File { path, quoted } => {
                let body = if *quoted { quote(git, target, &repo, draft, path)? } else { draft.body.clone() };
                payload.file_comments.push((draft.id.clone(), path.clone(), body));
            }
            Plan::Reply => payload.replies.push((draft.id.clone(), draft.reply_to.unwrap_or_default(), draft.body.clone())),
            Plan::Resolve => payload.resolves.push((draft.id.clone(), draft.thread_id.clone().unwrap_or_default())),
            Plan::Summary => payload.summary = Some((Some(draft.id.clone()), draft.body.clone())),
            Plan::Outdated => {}
        }
    }
    if let Some(text) = summary {
        let id = payload.summary.as_ref().and_then(|(id, _)| id.clone());
        payload.summary = Some((id, text.to_string()));
    }
    Ok(payload)
}

/// The quoted file-comment body for a line draft: the lines as they are on the current head
/// when they still exist there, otherwise as they were when the draft was written.
fn quote(git: &Git, target: &Target, repo: &str, draft: &Draft, path: &str) -> Result<String> {
    let end = draft.line.unwrap_or(1);
    let start = draft.start_line.unwrap_or(end);
    let (commit, side_commit) = match draft.side {
        Some(Side::Left) => (draft.base.as_str(), target.base),
        _ => (draft.commit.as_str(), target.head),
    };
    let current = remap_line(git, commit, side_commit, draft.path.as_deref().unwrap_or(path), start)?
        .zip(remap_line(git, commit, side_commit, draft.path.as_deref().unwrap_or(path), end)?);
    let (at, file, lo, hi) = match current {
        Some(((p, lo), (_, hi))) if lo <= hi => (side_commit, p, lo, hi),
        _ => (commit, draft.path.clone().unwrap_or_else(|| path.to_string()), start, end),
    };
    let lines = read_lines(git, at, &file, lo, hi)?;
    Ok(quoted_body(repo, at, &file, lo, hi, &lines, &draft.body))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub posted: usize,
    /// Draft id → why GitHub rejected it.
    pub failed: Vec<(String, String)>,
    /// Sent but the outcome is unknown (network); reconciled before the next submit.
    pub unknown: usize,
}

fn marker(token: &str) -> String {
    format!("<!-- wispydiff:{token} -->")
}

/// Sends the payload. Each draft's status is persisted before and after its request, so a
/// crash or network failure never loses a draft or posts it twice.
pub async fn send(github: &GitHubClient, cache: &Cache, target: &Target<'_>, payload: Payload, verdict: Verdict) -> Result<Outcome> {
    let mut outcome = Outcome::default();
    let pr = target.pr;

    let review_ids: Vec<String> = payload
        .review_comments
        .iter()
        .map(|(id, _)| id.clone())
        .chain(payload.summary.iter().filter_map(|(id, _)| id.clone()))
        .collect();
    let has_summary = payload.summary.as_ref().is_some_and(|(_, text)| !text.trim().is_empty());
    if !payload.review_comments.is_empty() || has_summary || verdict != Verdict::Comment {
        let token = new_id();
        let text = payload.summary.as_ref().map(|(_, t)| t.trim().to_string()).unwrap_or_default();
        let body = format!("{text}\n\n{}", marker(&token)).trim_start().to_string();
        set_status(cache, &review_ids, DraftStatus::Posting, None, Some(&token))?;
        let comments: Vec<NewReviewComment> = payload.review_comments.iter().map(|(_, c)| c.clone()).collect();
        match github.create_review(pr, target.head, &body, verdict, &comments).await {
            Ok(_) => {
                set_status(cache, &review_ids, DraftStatus::Posted, None, Some(&token))?;
                outcome.posted += review_ids.len().max(1);
            }
            Err(err) => record_failure(cache, &review_ids, err, &mut outcome)?,
        }
    }

    for (id, path, body) in &payload.file_comments {
        set_status(cache, std::slice::from_ref(id), DraftStatus::Posting, None, Some(id))?;
        let body = format!("{body}\n\n{}", marker(id));
        let result = github.create_file_comment(pr, target.head, path, &body).await;
        settle(cache, id, result.map(|_| ()), &mut outcome)?;
    }
    for (id, comment, body) in &payload.replies {
        set_status(cache, std::slice::from_ref(id), DraftStatus::Posting, None, Some(id))?;
        let body = format!("{body}\n\n{}", marker(id));
        let result = github.reply(pr, *comment, &body).await;
        settle(cache, id, result.map(|_| ()), &mut outcome)?;
    }
    for (id, thread) in &payload.resolves {
        // Resolving is idempotent: an unknown outcome is simply retried next time.
        match github.resolve_thread(thread).await {
            Ok(()) => {
                set_status(cache, std::slice::from_ref(id), DraftStatus::Posted, None, None)?;
                outcome.posted += 1;
            }
            Err(Error::GitHub(message)) => {
                set_status(cache, std::slice::from_ref(id), DraftStatus::Failed, Some(&message), None)?;
                outcome.failed.push((id.clone(), message));
            }
            Err(_) => outcome.unknown += 1,
        }
    }
    Ok(outcome)
}

fn settle(cache: &Cache, id: &str, result: Result<()>, outcome: &mut Outcome) -> Result<()> {
    match result {
        Ok(()) => {
            set_status(cache, &[id.to_string()], DraftStatus::Posted, None, Some(id))?;
            outcome.posted += 1;
            Ok(())
        }
        Err(err) => record_failure(cache, &[id.to_string()], err, outcome),
    }
}

/// A rejected request is a definite failure; anything else (network) leaves the drafts
/// `Posting` so the next submit checks GitHub before resending.
fn record_failure(cache: &Cache, ids: &[String], err: Error, outcome: &mut Outcome) -> Result<()> {
    match err {
        Error::GitHub(message) => {
            set_status(cache, ids, DraftStatus::Failed, Some(&message), None)?;
            outcome.failed.extend(ids.iter().map(|id| (id.clone(), message.clone())));
        }
        _ => outcome.unknown += ids.len(),
    }
    Ok(())
}

fn set_status(cache: &Cache, ids: &[String], status: DraftStatus, error: Option<&str>, token: Option<&str>) -> Result<()> {
    for id in ids {
        if let Some(mut draft) = cache.draft(id)? {
            draft.status = status;
            draft.error = error.map(str::to_string);
            if token.is_some() {
                draft.token = token.map(str::to_string);
            }
            draft.updated_at = now();
            cache.put_draft(&draft)?;
        }
    }
    Ok(())
}

/// Settles drafts left `Posting` by an interrupted submit: posted if GitHub has their marker,
/// otherwise back to `Draft` so they're sent again.
pub async fn reconcile(github: &GitHubClient, cache: &Cache, pr: &PrRef, drafts: &[Draft]) -> Result<()> {
    let posting: Vec<&Draft> = drafts.iter().filter(|d| d.status == DraftStatus::Posting).collect();
    if posting.is_empty() {
        return Ok(());
    }
    let mut seen = github.review_bodies(pr).await?;
    seen.extend(github.review_comment_bodies(pr).await?);
    for draft in posting {
        let found = draft.token.as_deref().is_some_and(|token| seen.iter().any(|body| body.contains(&marker(token))));
        let status = if found { DraftStatus::Posted } else { DraftStatus::Draft };
        set_status(cache, std::slice::from_ref(&draft.id), status, None, None)?;
    }
    Ok(())
}
