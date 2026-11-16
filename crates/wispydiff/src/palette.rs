//! ⌘K: open a PR by URL / `owner/repo#N`, or run a `>` command. While typing it suggests PRs:
//! the ones the app knows (inbox, the open stack, recent ones) at once, then GitHub's open PRs
//! matching the words (or the repo's latest, after `owner/repo#`) in the repos it knows.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{IndentInline, Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, Render, SharedString, Subscription,
    Task, Window,
};

use crate::ui::{floating, palette, u};

gpui_kit::actions!(palette, [SelectNext, SelectPrev, Close]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Palette");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("escape", Close, ctx),
    ]);
}

/// A palette action, shown when the input starts with ">".
#[derive(Clone)]
pub struct Command {
    pub id: SharedString,
    pub label: SharedString,
    /// Commands with a keyword take the rest of the input as their argument (`> ignore src/**`).
    pub keyword: Option<&'static str>,
    pub run: Rc<dyn Fn(&str, &mut Window, &mut App)>,
}

impl Command {
    pub fn new(id: &str, label: impl Into<SharedString>, run: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        Command { id: SharedString::from(id.to_string()), label: label.into(), keyword: None, run: Rc::new(run) }
    }

    pub fn with_keyword(mut self, keyword: &'static str) -> Self {
        self.keyword = Some(keyword);
        self
    }
}

/// A PR the palette can suggest: `owner/repo#N`, its title, and where it's known from.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub title: String,
    pub source: &'static str,
}

pub enum PaletteEvent {
    /// Open this PR (URL, `owner/repo#N`, or a recent label).
    Open(String),
    Close,
}

struct Match {
    id: SharedString,
    label: SharedString,
    /// A PR's title.
    detail: Option<SharedString>,
    /// Where a suggestion comes from ("inbox", "GitHub", …).
    source: Option<&'static str>,
    run: MatchRun,
}

enum MatchRun {
    Open(String),
    /// Puts this text in the input (a repo: `owner/repo#`), to go on typing.
    Fill(String),
    Command(Rc<dyn Fn(&str, &mut Window, &mut App)>, String),
}

/// GitHub's answer for a query, and whether one is on its way.
#[derive(Default)]
struct Remote {
    query: String,
    results: Vec<Suggestion>,
    pending: bool,
    failed: bool,
}

pub struct CommandPalette {
    input: Entity<InputState>,
    commands: Vec<Command>,
    recents: Vec<String>,
    known: Vec<Suggestion>,
    backend: Option<Arc<crate::backend::Backend>>,
    remote: Remote,
    _search: Option<Task<()>>,
    selected: usize,
    pub busy: Option<String>,
    pub error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(commands: Vec<Command>, recents: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search PRs, paste a URL or owner/repo#123 · > for commands"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                this.selected = 0;
                this.search_soon(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.submit(window, cx),
            _ => {}
        })];
        input.update(cx, |input, cx| input.focus(window, cx));
        CommandPalette {
            input,
            commands,
            recents,
            known: Vec::new(),
            backend: None,
            remote: Remote::default(),
            _search: None,
            selected: 0,
            busy: None,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// PRs to suggest (inbox, the open stack), and GitHub to ask for more.
    pub fn with_suggestions(mut self, known: Vec<Suggestion>, backend: Option<Arc<crate::backend::Backend>>) -> Self {
        self.known = known;
        self.backend = backend;
        self
    }

    /// The repos the user works in: the known PRs' and recent ones', most recent first.
    fn repos(&self) -> Vec<String> {
        let mut repos: Vec<String> = Vec::new();
        for label in self.recents.iter().chain(self.known.iter().map(|k| &k.label)) {
            if let Some((repo, _)) = label.rsplit_once('#') {
                if repo.contains('/') && !repos.iter().any(|r| r == repo) {
                    repos.push(repo.to_string());
                }
            }
        }
        repos
    }

    /// Asks GitHub after a pause in typing (each keystroke restarts the wait).
    fn search_soon(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx).trim().to_string();
        let Some(backend) = self.backend.clone() else { return };
        if query.starts_with('>') || query.len() < 2 || looks_like_pr(&query) || query.contains("://") {
            self._search = None;
            self.remote = Remote::default();
            return;
        }
        if query == self.remote.query {
            return;
        }
        // `owner/repo#…` (or just `owner/repo`): that repo's latest open PRs.
        let (text, repos) = match query.split_once('#').map(|(r, _)| r).filter(|r| r.contains('/')).or_else(|| (query.contains('/') && !query.contains(' ')).then_some(query.as_str())) {
            Some(repo) => (String::new(), vec![repo.to_string()]),
            None => {
                let mut repos = self.repos();
                repos.truncate(5);
                if repos.is_empty() {
                    (format!("{query} involves:@me"), repos)
                } else {
                    (query.clone(), repos)
                }
            }
        };
        self.remote.pending = true;
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let result = backend.search_prs(text, repos).await;
            let _ = this.update(cx, |this, cx| {
                this.remote = match result {
                    Ok(prs) => Remote {
                        query,
                        results: prs.into_iter().map(|p| Suggestion { label: format!("{}#{}", p.repo, p.number), title: p.title, source: "GitHub" }).collect(),
                        pending: false,
                        failed: false,
                    },
                    Err(_) => Remote { query, results: Vec::new(), pending: false, failed: true },
                };
                cx.notify();
            });
        }));
    }

    pub fn set_commands(&mut self, commands: Vec<Command>, cx: &mut Context<Self>) {
        self.commands = commands;
        cx.notify();
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    fn matches(&self, cx: &App) -> Vec<Match> {
        let query = self.query(cx);
        if let Some(rest) = query.strip_prefix('>') {
            let q = rest.trim().to_lowercase();
            return self
                .commands
                .iter()
                .filter_map(|c| {
                    let keyed = c.keyword.filter(|k| q.starts_with(&format!("{k} ")));
                    if keyed.is_none() && !fuzzy(&c.label.to_lowercase(), &q) {
                        return None;
                    }
                    let argument = match keyed {
                        Some(k) => rest.trim()[k.len() + 1..].to_string(),
                        None => String::new(),
                    };
                    let label: SharedString =
                        if argument.is_empty() { c.label.clone() } else { format!("{} “{argument}”", c.label).into() };
                    Some(Match { id: c.id.clone(), label, detail: None, source: None, run: MatchRun::Command(c.run.clone(), argument) })
                })
                .take(10)
                .collect();
        }
        let q = query.trim();
        let title_of = |label: &str| self.known.iter().find(|k| k.label == label).map(|k| k.title.clone()).unwrap_or_default();
        let recents = self.recents.iter().map(|label| Suggestion { label: label.clone(), title: title_of(label), source: "recent" });
        let remote = self.remote.results.iter().filter(|_| self.remote.query == q).cloned();
        let candidates: Vec<Suggestion> = recents.chain(self.known.iter().cloned()).chain(remote).collect();
        suggest(q, &candidates, &self.repos())
            .into_iter()
            .map(|(suggestion, fill)| Match {
                id: suggestion.label.clone().into(),
                label: if fill { format!("{}#…", suggestion.label).into() } else { suggestion.label.clone().into() },
                detail: (!suggestion.title.is_empty()).then(|| suggestion.title.clone().into()),
                source: Some(suggestion.source),
                run: if fill { MatchRun::Fill(format!("{}#", suggestion.label)) } else { MatchRun::Open(suggestion.label) },
            })
            .collect()
    }

    /// Tab: puts the highlighted suggestion in the input (a repo: `owner/repo#`).
    fn complete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut matches = self.matches(cx);
        if self.selected >= matches.len() {
            return;
        }
        let text = match matches.swap_remove(self.selected).run {
            MatchRun::Open(label) => label,
            MatchRun::Fill(text) => text,
            MatchRun::Command(..) => return,
        };
        self.input.update(cx, |input, cx| input.set_value(text, window, cx));
        self.selected = 0;
        self.search_soon(cx);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let command_mode = query.starts_with('>');
        if !command_mode && looks_like_pr(&query) {
            cx.emit(PaletteEvent::Open(query.trim().to_string()));
            return;
        }
        let mut matches = self.matches(cx);
        if self.selected < matches.len() {
            let chosen = matches.swap_remove(self.selected);
            self.run(chosen, window, cx);
        } else if !command_mode && !query.trim().is_empty() {
            cx.emit(PaletteEvent::Open(query.trim().to_string()));
        }
    }

    fn run(&mut self, chosen: Match, window: &mut Window, cx: &mut Context<Self>) {
        match chosen.run {
            MatchRun::Open(label) => cx.emit(PaletteEvent::Open(label)),
            MatchRun::Fill(text) => {
                self.input.update(cx, |input, cx| input.set_value(text, window, cx));
                self.selected = 0;
                self.search_soon(cx);
                cx.notify();
            }
            MatchRun::Command(run, argument) => {
                cx.emit(PaletteEvent::Close);
                run(&argument, window, cx);
            }
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn close(&mut self, _: &Close, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Close);
    }
}

/// The suggestions for `query`, best first, at most 10: PRs whose `repo#number title` has every
/// word of it (a number matches the PR's number from its start), repos to go on in (`true`: a
/// fill-in, `owner/repo#`), and a bare number as `repo#number` in each of `repos`. An empty
/// query lists the recent and known PRs. Duplicates keep their first (most local) source.
pub fn suggest(query: &str, candidates: &[Suggestion], repos: &[String]) -> Vec<(Suggestion, bool)> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<(Suggestion, bool, u8)> = Vec::new();
    let push = |s: Suggestion, fill: bool, rank: u8, out: &mut Vec<(Suggestion, bool, u8)>| {
        if !out.iter().any(|(o, f, _)| o.label == s.label && *f == fill) {
            out.push((s, fill, rank));
        }
    };
    if q.is_empty() {
        for c in candidates {
            push(c.clone(), false, 0, &mut out);
        }
    } else {
        let words: Vec<&str> = q.split_whitespace().collect();
        // `owner/repo#12`: the number typed so far narrows that repo's PRs.
        let (repo_part, number_part) = match q.split_once('#') {
            Some((r, n)) if n.chars().all(|c| c.is_ascii_digit()) => (Some(r), n),
            _ => (None, ""),
        };
        for c in candidates {
            let label = c.label.to_lowercase();
            let (repo, number) = label.rsplit_once('#').unwrap_or((label.as_str(), ""));
            let rank = if let Some(r) = repo_part {
                (repo.contains(r) && number.starts_with(number_part)).then_some(0)
            } else {
                let haystack = format!("{label} {}", c.title.to_lowercase());
                words
                    .iter()
                    .all(|w| haystack.contains(w) || number.starts_with(w.trim_start_matches('#')))
                    .then(|| if label.starts_with(&q) || number.starts_with(q.trim_start_matches('#')) { 0 } else { 1 })
            };
            if let Some(rank) = rank {
                push(c.clone(), false, rank, &mut out);
            }
        }
        if q.chars().all(|c| c.is_ascii_digit()) {
            for repo in repos {
                push(Suggestion { label: format!("{repo}#{q}"), title: String::new(), source: "open" }, false, 1, &mut out);
            }
        }
        if !q.contains('#') && !q.contains(' ') {
            for repo in repos.iter().filter(|r| r.to_lowercase().contains(&q)) {
                push(Suggestion { label: repo.clone(), title: String::new(), source: "repo" }, true, 2, &mut out);
            }
        }
    }
    out.sort_by_key(|(_, _, rank)| *rank);
    out.into_iter().take(10).map(|(s, fill, _)| (s, fill)).collect()
}

/// `#123` at the end, or a `/pull/123` URL.
pub fn looks_like_pr(s: &str) -> bool {
    let t = s.trim_end();
    let hash = t.rfind('#').is_some_and(|at| at + 1 < t.len() && t[at + 1..].chars().all(|c| c.is_ascii_digit()));
    let pull = t.find("/pull/").is_some_and(|at| t[at + 6..].chars().next().is_some_and(|c| c.is_ascii_digit()));
    hash || pull
}

/// The characters of `needle` appear in `haystack` in order.
pub fn fuzzy(haystack: &str, needle: &str) -> bool {
    let mut wanted = needle.chars().peekable();
    for ch in haystack.chars() {
        if wanted.peek() == Some(&ch) {
            wanted.next();
        }
    }
    wanted.peek().is_none()
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let matches = self.matches(cx);
        let command_mode = self.query(cx).starts_with('>');
        let selected = self.selected.min(matches.len().saturating_sub(1));
        floating(620., cx)
            .key_context("Palette")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::close))
            // The input takes ↑/↓/Esc itself: the palette needs them first.
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveDown, window, cx| {
                cx.stop_propagation();
                this.select_next(&SelectNext, window, cx)
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveUp, window, cx| {
                cx.stop_propagation();
                this.select_prev(&SelectPrev, window, cx)
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                cx.stop_propagation();
                this.close(&Close, window, cx)
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                cx.stop_propagation();
                this.complete(window, cx)
            }))
            .child(div().p(u(8.)).child(Input::new(&self.input).appearance(false).text_size(u(15.))))
            .when_some(self.busy.clone(), |d, busy| {
                d.child(div().px(u(14.)).pb(u(10.)).text_size(u(12.)).text_color(p.fg_muted).child(format!("Loading {busy}…")))
            })
            .when(self.busy.is_none(), |d| {
                d.when_some(self.error.clone(), |d, error| {
                    d.child(div().px(u(14.)).pb(u(10.)).text_size(u(12.)).text_color(p.del_fg).child(error))
                })
            })
            .when(self.busy.is_none() && !matches.is_empty(), |d| {
                d.child(div().border_t_1().border_color(p.border).py(u(4.)).children(matches.into_iter().enumerate().map(
                    |(i, m)| {
                        let id = m.id.clone();
                        let (label, detail, source) = (m.label.clone(), m.detail.clone(), m.source);
                        let chosen = std::rc::Rc::new(std::cell::RefCell::new(Some(m)));
                        div()
                            .id(SharedString::from(format!("palette-{id}")))
                            .px(u(14.))
                            .py(u(6.))
                            .text_size(u(13.))
                            .when(command_mode, |d| d.text_color(p.fg))
                            .when(i == selected, |d| d.bg(p.selection))
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.selected != i {
                                    this.selected = i;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(m) = chosen.borrow_mut().take() {
                                    this.run(m, window, cx);
                                }
                            }))
                            .flex()
                            .items_baseline()
                            .gap(u(10.))
                            .whitespace_nowrap()
                            .child(div().flex_none().child(label))
                            .when_some(detail, |d, detail| {
                                d.child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().text_color(p.fg_muted).child(detail))
                            })
                            .when_some(source, |d, source| {
                                d.child(div().flex_none().ml_auto().text_size(u(11.)).text_color(p.fg_muted).child(source))
                            })
                    },
                )))
            })
            .when(self.busy.is_none() && !command_mode && (self.remote.pending || self.remote.failed), |d| {
                let note = if self.remote.pending { "Searching GitHub…" } else { "GitHub search unavailable (offline?)" };
                d.child(div().px(u(14.)).py(u(6.)).border_t_1().border_color(p.border).text_size(u(11.)).text_color(p.fg_muted).child(note))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_pr_references() {
        assert!(looks_like_pr("owner/repo#12"));
        assert!(looks_like_pr("https://github.com/o/r/pull/7/files"));
        assert!(!looks_like_pr("owner/repo"));
        assert!(!looks_like_pr("#"));
    }

    fn s(label: &str, title: &str, source: &'static str) -> Suggestion {
        Suggestion { label: label.into(), title: title.into(), source }
    }

    #[test]
    fn suggests_known_prs_numbers_and_repos() {
        let known = [
            s("acme/shop#19117", "Filter picking profiles by pickable units", "inbox"),
            s("acme/shop#88", "Fix carrier label", "recent"),
            s("acme/app#5", "Pickable units in the app", "GitHub"),
        ];
        let repos = ["acme/shop".to_string(), "acme/app".to_string()];
        let labels = |q: &str| suggest(q, &known, &repos).into_iter().map(|(s, fill)| (s.label, fill)).collect::<Vec<_>>();
        // Every word, in the label or the title.
        assert_eq!(labels("pickable units"), [("acme/shop#19117".into(), false), ("acme/app#5".into(), false)]);
        // A number: the PR whose number starts with it, and that number in each known repo.
        assert_eq!(labels("1911"), [("acme/shop#19117".into(), false), ("acme/shop#1911".into(), false), ("acme/app#1911".into(), false)]);
        // A repo to go on in; after `#`, that repo's PRs by number.
        assert_eq!(labels("app"), [("acme/app#5".into(), false), ("acme/app".into(), true)]);
        assert_eq!(labels("acme/shop#8"), [("acme/shop#88".into(), false)]);
        // Nothing typed: what's known, as given.
        assert_eq!(labels("").len(), 3);
        // The same PR from two sources once.
        let twice = [s("acme/shop#88", "Fix", "recent"), s("acme/shop#88", "Fix", "GitHub")];
        assert_eq!(suggest("fix", &twice, &[]).len(), 1);
    }

    #[test]
    fn fuzzy_matches_in_order() {
        assert!(fuzzy("hide whitespace changes", "hwc"));
        assert!(!fuzzy("theme", "mt"));
        assert!(fuzzy("anything", ""));
    }
}
